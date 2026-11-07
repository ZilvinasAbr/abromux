//! GitLab merge requests kept fresh in the background, so any of them opens
//! at once in a GitLab tab (the `abromux-gitlab` plugin), and the right-hand
//! panel listing them (`panel.rs`).
//!
//! What's kept and when it's dropped is in `model.rs`. Keeping it fresh is
//! two steps: a cheap GraphQL check of many merge requests in one request
//! (`api::light_query`), often; and, only when that shows a change, the
//! full load of the merge request as its tab shows it, recorded on disk
//! (`abromux_gitlab::cache::fetch`). A tab showing it takes the new load in
//! place; one opened later starts from it. How often each is checked depends
//! on what it is (on screen, pipeline running, yours…; see
//! [`Store::cadence`]), all stretched by the rate-limit slowdown in
//! `throttle.rs`.
//!
//! Requests run one at a time in two lanes, so a big merge request loading
//! in the background never holds up the one on screen:
//! - **front**: what you asked to refresh, what's on screen, and the cheap
//!   checks;
//! - **back**: everything else's load.
//!
//! The host and token are the GitLab tab's: the one signed in to there (the
//! Keychain), unless `gitlab.host` says which. The list is kept in
//! `gitlab-cache/index.json` under Application Support, the recorded loads
//! in `gitlab-cache/reviews/`.

pub mod api;
pub mod model;
mod panel;
pub mod remote;
pub mod throttle;

use crate::settings::{GitlabSettings, Settings};
use abromux_gitlab::{api::Client, cache, review::Review};
use api::{Error, Lists};
use futures::{FutureExt as _, StreamExt as _, channel::mpsc};
use gpui::{App, AppContext as _, Context, Entity, Global};
use model::{Light, Mr, MrKey, Recorded};
use remote::Place;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    rc::Rc,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use throttle::{Prio, Throttle};

/// At most this many merge requests per cheap check.
const BATCH: usize = 40;
/// A failed fetch isn't tried again for this long (times the slowdown).
const RETRY: Duration = Duration::from_secs(60);
/// While the app isn't in front, everything is checked this much less often.
const INACTIVE_FACTOR: u32 = 3;
/// How often the host and token are looked up again (a sign-in in a tab).
const WHO_EVERY: Duration = Duration::from_secs(20);

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lane {
    Front,
    Back,
}

enum Job {
    Light { keys: Vec<MrKey>, lists: Option<Lists>, min_fields: bool },
    /// Load it as its tab would, recorded; with its project's id if known.
    Review(MrKey, Option<u64>),
}

/// One per finished job, handed straight back; its size doesn't matter.
#[allow(clippy::large_enum_variant)]
enum Done {
    Light {
        keys: Vec<MrKey>,
        lists: Option<Lists>,
        result: Result<api::Checked, Error>,
    },
    Review(MrKey, Instant, Result<(Review, u64), Error>),
}

/// What a job needs, taken on the main thread.
#[derive(Clone)]
struct Snap {
    host: String,
    token: String,
    throttle: Arc<Mutex<Throttle>>,
}

#[derive(Serialize, Deserialize, Default)]
struct Index {
    host: String,
    mrs: Vec<Mr>,
}

pub struct Store {
    pub mrs: BTreeMap<MrKey, Mr>,
    /// The GitLab the cache is for (`https://gitlab.com`); empty until
    /// looked up. Another one empties it.
    pub host: String,
    token: Option<String>,
    /// None until looked up.
    pub signed_in: Option<bool>,
    who_at: Option<Instant>,
    who_busy: bool,
    /// Workspace id → its merge requests, from its checked-out branches.
    pub pinned: HashMap<usize, Vec<MrKey>>,
    /// Workspace id → where to look, as last given.
    places: HashMap<usize, Vec<Place>>,
    /// Workspace id → projects and branches found there.
    found: HashMap<usize, Vec<(String, Vec<String>)>>,
    found_at: Option<Instant>,
    finding: bool,
    /// Nothing is dropped for not being pinned before the first look.
    found_once: bool,
    /// Shown in a GitLab tab now, on screen.
    pub on_screen: HashSet<MrKey>,
    /// Shown in a GitLab tab anywhere (`AbroMux::sync_mr_views`).
    pub open: HashSet<MrKey>,
    /// The tabs were looked at once: until then nothing's dropped for not
    /// being open.
    views_once: bool,
    app_active: bool,
    /// When each was last checked.
    checked: HashMap<MrKey, Instant>,
    lists_checked: Option<Instant>,
    /// The lists came back at least once, so "Mine" is real.
    pub lists_ok: bool,
    /// Refreshes asked for (Refresh, `abromux mr refresh`), in order.
    urgent: VecDeque<MrKey>,
    busy: HashSet<MrKey>,
    light_busy: bool,
    failed: HashMap<MrKey, Instant>,
    pub throttle: Arc<Mutex<Throttle>>,
    /// Why the last request failed, until one succeeds.
    pub error: Option<String>,
    /// The GitLab is too old for some of `api::FIELDS`.
    min_fields: bool,
    /// The last `cache::Shared` load taken in (`take_shared`).
    shared_seen: u64,
    /// Recordings nothing here knows of were deleted (once, at the start).
    pruned: bool,
    dirty: bool,
    kick: [mpsc::UnboundedSender<()>; 2],
}

struct StoreGlobal(Entity<Store>);
impl Global for StoreGlobal {}

pub fn store(cx: &App) -> Entity<Store> {
    cx.global::<StoreGlobal>().0.clone()
}

fn settings(cx: &App) -> &GitlabSettings {
    &cx.global::<Settings>().gitlab
}

/// Signed in (or not looked up yet), so the panel lists rather than asks.
pub fn configured(cx: &App) -> bool {
    store(cx).read(cx).signed_in != Some(false)
}

fn cache_dir() -> std::path::PathBuf {
    crate::channel::support_dir().join("gitlab-cache")
}

fn index_path() -> std::path::PathBuf {
    cache_dir().join("index.json")
}

fn load_index() -> Index {
    std::fs::read(index_path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// `gitlab.com` as the plugin writes hosts: `https://gitlab.com`.
pub fn base_url(host: &str) -> String {
    let h = host.trim().trim_end_matches('/');
    if h.contains("://") {
        h.to_string()
    } else {
        format!("https://{h}")
    }
}

/// Which GitLab, and its token: `gitlab.host`, else the one a GitLab tab
/// signs in to (`abromux_gitlab::view`). Blocks (the Keychain).
fn who(setting: Option<String>) -> (String, Option<String>) {
    let host = match setting.filter(|h| !h.trim().is_empty()) {
        Some(h) => base_url(&h),
        None if std::env::var_os("ABROMUX_GITLAB_TOKEN").is_some() => abromux_gitlab::DEFAULT_HOST.into(),
        None => abromux_gitlab::auth::saved_host().unwrap_or_else(|| abromux_gitlab::DEFAULT_HOST.into()),
    };
    let token = abromux_gitlab::auth::token(&host);
    (cache::host_key(&host), token)
}

fn limits(s: &GitlabSettings) -> cache::Limits {
    cache::Limits {
        file_bytes: s.max_file_diff_kb.max(1) as usize * 1024,
        mr_bytes: s.max_mr_diff_mb.max(1) as usize * 1024 * 1024,
        mr_files: s.max_mr_diff_files.max(1) as usize,
    }
}

pub fn init(cx: &mut App) {
    cache::init(cache_dir());
    abromux_gitlab::local::init(crate::channel::support_dir().join("local-diffs"));
    cache::set_limits(Some(limits(settings(cx))));
    let index = load_index();
    let (front_tx, front_rx) = mpsc::unbounded();
    let (back_tx, back_rx) = mpsc::unbounded();
    let host = index.host.clone();
    let mrs: BTreeMap<MrKey, Mr> = index.mrs.into_iter().map(|m| (m.key.clone(), m)).collect();
    // Links to these find their project at once.
    for m in mrs.values() {
        if let Some(id) = m.project_id() {
            cache::learn_project(cx, &host, &m.key.project, id);
        }
    }
    let entity = cx.new(|_| Store {
        mrs,
        host,
        token: None,
        signed_in: None,
        who_at: None,
        who_busy: false,
        pinned: HashMap::new(),
        places: HashMap::new(),
        found: HashMap::new(),
        found_at: None,
        finding: false,
        found_once: false,
        on_screen: HashSet::new(),
        open: HashSet::new(),
        views_once: false,
        app_active: true,
        checked: HashMap::new(),
        lists_checked: None,
        lists_ok: false,
        urgent: VecDeque::new(),
        busy: HashSet::new(),
        light_busy: false,
        failed: HashMap::new(),
        throttle: Default::default(),
        error: None,
        min_fields: false,
        shared_seen: 0,
        pruned: false,
        dirty: false,
        kick: [front_tx, back_tx],
    });
    cx.set_global(StoreGlobal(entity.clone()));
    // What tabs load goes into the list too.
    cx.observe_global::<cache::Shared>(|cx| store(cx).update(cx, |s, cx| s.take_shared(cx)))
        .detach();
    spawn_lane(Lane::Front, front_rx, cx);
    spawn_lane(Lane::Back, back_rx, cx);
    // Saving, and looking up the host and token.
    cx.spawn(async move |cx| {
        loop {
            let _ = cx.update(|cx| {
                cache::set_limits(Some(limits(settings(cx))));
                store(cx).update(cx, |s, cx| s.look_up_who(cx));
            });
            let Ok(Some(index)) = cx.update(|cx| {
                store(cx).update(cx, |s, _| {
                    s.dirty.then(|| {
                        s.dirty = false;
                        Index {
                            host: s.host.clone(),
                            mrs: s.mrs.values().cloned().collect(),
                        }
                    })
                })
            }) else {
                cx.background_executor().timer(Duration::from_secs(5)).await;
                continue;
            };
            cx.background_executor()
                .spawn(async move {
                    let _ = std::fs::create_dir_all(cache_dir());
                    if let Ok(bytes) = serde_json::to_vec(&index) {
                        let tmp = index_path().with_extension("tmp");
                        if std::fs::write(&tmp, bytes).is_ok() {
                            let _ = std::fs::rename(tmp, index_path());
                        }
                    }
                })
                .await;
            cx.background_executor().timer(Duration::from_secs(5)).await;
        }
    })
    .detach();
}

fn spawn_lane(lane: Lane, mut wake: mpsc::UnboundedReceiver<()>, cx: &mut App) {
    cx.spawn(async move |cx| {
        loop {
            let job = cx
                .update(|cx| {
                    let gs = settings(cx).clone();
                    store(cx).update(cx, |s, _| {
                        let snap = s.snap()?;
                        s.next_job(lane, &gs, Instant::now()).map(|j| (j, snap))
                    })
                })
                .ok()
                .flatten();
            match job {
                Some((job, snap)) => {
                    let done = cx.background_executor().spawn(async move { run(job, snap) }).await;
                    let _ = cx.update(|cx| store(cx).update(cx, |s, cx| s.finish(done, cx)));
                }
                None => {
                    let timer = cx.background_executor().timer(Duration::from_secs(1));
                    futures::select! {
                        _ = timer.fuse() => {}
                        _ = wake.next() => {}
                    }
                    // Several kicks at once are one wake-up.
                    while wake.try_recv().is_ok() {}
                }
            }
        }
    })
    .detach();
}

fn run(job: Job, snap: Snap) -> Done {
    let paced = api::Paced {
        inner: Arc::new(abromux_gitlab::api::Curl),
        throttle: snap.throttle.clone(),
    };
    let c = Client::new(&snap.host, &snap.token, Arc::new(paced));
    match job {
        Job::Light { keys, lists, min_fields } => {
            let fields = if min_fields { api::MIN_FIELDS } else { api::FIELDS };
            let q = api::light_query(&keys, lists.as_ref(), fields);
            let result = api::graphql(&c, &q).map(|data| api::parse_checked(&data, &keys, lists.as_ref()));
            Done::Light { keys, lists, result }
        }
        Job::Review(k, project) => {
            let started = Instant::now();
            let r = (|| {
                let p = match project {
                    Some(p) => p,
                    None => c.project_id(&k.project)?,
                };
                cache::fetch(&c, p, k.iid)
            })()
            .map_err(|e| Error::of(&e));
            Done::Review(k, started, r)
        }
    }
}

impl Store {
    fn snap(&self) -> Option<Snap> {
        Some(Snap {
            host: self.host.clone(),
            token: self.token.clone()?,
            throttle: self.throttle.clone(),
        })
    }

    /// Look up the host and token again, now and then: signing in in a tab,
    /// or `gitlab.host` changing, starts (or switches) the cache.
    fn look_up_who(&mut self, cx: &mut Context<Self>) {
        let setting = settings(cx).host.clone();
        let wanted = setting.as_deref().map(base_url).map(|h| cache::host_key(&h));
        let switched = wanted.as_ref().is_some_and(|h| *h != self.host);
        if self.who_busy || (!switched && self.who_at.is_some_and(|t| t.elapsed() < WHO_EVERY)) {
            return;
        }
        self.who_busy = true;
        cx.spawn(async move |this, cx| {
            let (host, token) = cx.background_executor().spawn(async move { who(setting) }).await;
            let _ = this.update(cx, |s, cx| {
                s.who_busy = false;
                s.who_at = Some(Instant::now());
                if s.host != host {
                    s.reset(host, cx);
                }
                let changed = s.token != token;
                s.signed_in = Some(token.is_some());
                s.token = token;
                if changed {
                    s.error = None;
                    s.wake();
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn reset(&mut self, host: String, cx: &mut Context<Self>) {
        self.host = host;
        self.mrs.clear();
        self.pinned.clear();
        self.checked.clear();
        self.lists_checked = None;
        self.lists_ok = false;
        self.error = None;
        self.found_at = None;
        self.pruned = false;
        self.dirty = true;
        self.wake();
        cx.notify();
    }

    fn wake(&self) {
        for k in &self.kick {
            let _ = k.unbounded_send(());
        }
    }

    /// How urgent `m` is and how often (seconds) it's checked.
    fn cadence(&self, m: &Mr, recent_rank: Option<usize>, s: &GitlabSettings) -> (Prio, u32) {
        let pinned = self.pinned.values().any(|v| v.contains(&m.key));
        let (prio, secs) = if self.on_screen.contains(&m.key) {
            (Prio::OnScreen, s.on_screen_seconds)
        } else if m.light.as_ref().is_some_and(|l| l.pipeline_active()) {
            (Prio::Running, s.running_pipeline_seconds)
        } else if pinned
            || m.roles.any()
            || recent_rank.is_some_and(|r| r < s.frequent_recent as usize)
        {
            (Prio::Mine, s.mine_seconds)
        } else {
            (Prio::Recent, s.recent_seconds)
        };
        let secs = if self.app_active { secs } else { secs * INACTIVE_FACTOR };
        (prio, secs)
    }

    fn recent_ranks(&self) -> HashMap<MrKey, usize> {
        let mut r: Vec<(u64, &MrKey)> = self
            .mrs
            .values()
            .filter_map(|m| m.inspected_at.map(|t| (t, &m.key)))
            .collect();
        r.sort_by(|a, b| b.cmp(a));
        r.into_iter().enumerate().map(|(i, (_, k))| (k.clone(), i)).collect()
    }

    fn failed_lately(&self, k: &MrKey, t: &Throttle, now: Instant) -> bool {
        self.failed
            .get(k)
            .is_some_and(|at| now.duration_since(*at) < RETRY.mul_f64(t.slow))
    }

    fn lists(&self, s: &GitlabSettings) -> Lists {
        let mut by_project: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for found in self.found.values() {
            for (p, bs) in found {
                let e = by_project.entry(p.clone()).or_default();
                for b in bs {
                    if !e.contains(b) {
                        e.push(b.clone());
                    }
                }
            }
        }
        Lists {
            mine: true,
            review: s.include_review_requests,
            assigned: s.include_assigned,
            branches: by_project.into_iter().collect(),
        }
    }

    fn next_job(&mut self, lane: Lane, s: &GitlabSettings, now: Instant) -> Option<Job> {
        let t_arc = self.throttle.clone();
        let mut t = t_arc.lock().unwrap();
        t.tick(now);
        let job = match lane {
            Lane::Front => self.front_job(s, &t, now),
            Lane::Back => self.back_job(s, &t, now),
        };
        drop(t);
        match &job {
            Some(Job::Light { keys, lists, .. }) => {
                self.light_busy = true;
                for k in keys {
                    self.checked.insert(k.clone(), now);
                }
                if lists.is_some() {
                    self.lists_checked = Some(now);
                }
            }
            Some(Job::Review(k, _)) => {
                self.busy.insert(k.clone());
            }
            None => {}
        }
        job
    }

    fn review_job(&self, k: &MrKey) -> Job {
        Job::Review(k.clone(), self.mrs.get(k).and_then(Mr::project_id))
    }

    fn front_job(&mut self, s: &GitlabSettings, t: &Throttle, now: Instant) -> Option<Job> {
        if !t.allows(Prio::Opened, now) {
            return None;
        }
        // 1. What you asked to refresh.
        if let Some(i) = self.urgent.iter().position(|k| !self.busy.contains(k)) {
            let k = self.urgent.remove(i).expect("found");
            return Some(self.review_job(&k));
        }
        // 2. What's on screen, when the cheap check saw it change.
        if t.allows(Prio::OnScreen, now) {
            let mut shown: Vec<&MrKey> = self.on_screen.iter().collect();
            shown.sort();
            for k in shown {
                let Some(m) = self.mrs.get(k) else { continue };
                if m.review_stale() && !self.busy.contains(k) && !self.failed_lately(k, t, now) {
                    return Some(self.review_job(k));
                }
            }
        }
        // 3. The cheap check.
        if self.light_busy {
            return None;
        }
        let lists_due = self.found_once
            && t.allows(Prio::Mine, now)
            && self.lists_checked.is_none_or(|c| {
                let secs = if self.app_active {
                    s.mine_seconds
                } else {
                    s.mine_seconds * INACTIVE_FACTOR
                };
                now.duration_since(c) >= t.interval(secs)
            });
        let ranks = self.recent_ranks();
        let pinned: HashSet<&MrKey> = self.pinned.values().flatten().collect();
        let mut due: Vec<(Prio, bool, MrKey)> = vec![];
        for m in self.mrs.values() {
            let (prio, secs) = self.cadence(m, ranks.get(&m.key).copied(), s);
            if !t.allows(prio, now) {
                continue;
            }
            // Yours and the workspaces' come back with the lists, which run
            // at their pace; only a faster one (on screen, pipeline
            // running) is asked about on its own.
            let listed = m.roles.any() || pinned.contains(&m.key);
            if listed && prio >= Prio::Mine {
                continue;
            }
            let every = t.interval(secs);
            let since = self.checked.get(&m.key).map(|c| now.duration_since(*c));
            let ripe = since.is_none_or(|d| d >= every);
            // Ones halfway there come along, so checks batch up.
            let half = since.is_none_or(|d| d >= every / 2);
            if half {
                due.push((prio, ripe, m.key.clone()));
            }
        }
        if !lists_due && !due.iter().any(|(_, ripe, _)| *ripe) {
            return None;
        }
        let lists = lists_due.then(|| self.lists(s));
        // Nor, in a check that has the lists, the ones they bring back.
        due.retain(|(prio, _, k)| {
            lists.is_none()
                || *prio <= Prio::OnScreen
                || !(self.mrs[k].roles.any() || pinned.contains(k))
        });
        due.sort_by_key(|(prio, ripe, _)| (*prio, !*ripe));
        let keys: Vec<MrKey> = due.into_iter().take(BATCH).map(|(.., k)| k).collect();
        if keys.is_empty() && lists.is_none() {
            return None;
        }
        Some(Job::Light {
            keys,
            lists,
            min_fields: self.min_fields,
        })
    }

    /// The load of whatever changed since its recording, most urgent first.
    fn back_job(&mut self, s: &GitlabSettings, t: &Throttle, now: Instant) -> Option<Job> {
        let ranks = self.recent_ranks();
        let mut todo: Vec<(Prio, &MrKey)> = vec![];
        for m in self.mrs.values() {
            if self.on_screen.contains(&m.key) || !m.wants_review() {
                continue;
            }
            let (prio, _) = self.cadence(m, ranks.get(&m.key).copied(), s);
            let prio = prio.max(Prio::Background);
            if t.allows(prio, now) && !self.busy.contains(&m.key) && !self.failed_lately(&m.key, t, now) {
                todo.push((prio, &m.key));
            }
        }
        todo.sort();
        let k = todo.first()?.1.clone();
        Some(self.review_job(&k))
    }

    fn finish(&mut self, done: Done, cx: &mut Context<Self>) {
        let now = Instant::now();
        let secs = now_secs();
        match done {
            Done::Light { keys, lists, result } => {
                self.light_busy = false;
                match result {
                    Ok(c) => {
                        self.error = None;
                        self.apply_checked(c, lists.is_some(), now, secs);
                        let host = self.host.clone();
                        for m in self.mrs.values() {
                            if let Some(id) = m.project_id() {
                                cache::learn_project(cx, &host, &m.key.project, id);
                            }
                        }
                    }
                    Err(Error::Other(m)) if m.contains("doesn't exist on type") && !self.min_fields => {
                        self.min_fields = true;
                        for k in &keys {
                            self.checked.remove(k);
                        }
                        self.lists_checked = None;
                    }
                    Err(e) => self.failed_with(e),
                }
            }
            Done::Review(k, started, result) => {
                self.busy.remove(&k);
                match result {
                    Ok((r, bytes)) => {
                        self.error = None;
                        self.failed.remove(&k);
                        self.took(&k, &r, bytes, now, secs);
                        cache::put(cx, &self.host.clone(), Rc::new(r), started, bytes, false);
                        self.recompute_pinned();
                    }
                    Err(Error::NotFound) => {
                        if let Some(m) = self.mrs.get_mut(&k) {
                            m.gone = true;
                        }
                    }
                    Err(e) => {
                        self.failed.insert(k, now);
                        self.failed_with(e);
                    }
                }
            }
        }
        self.evict(cx);
        self.dirty = true;
        cx.notify();
    }

    /// `r` (of `k`, recorded in `bytes`) is the latest: what the cheap check
    /// knows of it, and that its recording is current.
    fn took(&mut self, k: &MrKey, r: &Review, bytes: u64, now: Instant, secs: u64) {
        let m = self.mrs.entry(k.clone()).or_insert_with(|| Mr::new(k.clone()));
        let light = Light::of_review(r, m.light.as_ref());
        m.review = Some(Recorded {
            signature: light.signature(),
            bytes: if bytes > 0 { bytes } else { m.review.as_ref().map_or(0, |d| d.bytes) },
        });
        m.light = Some(light);
        m.review_skip = None;
        m.gone = false;
        m.fetched_at = secs;
        self.checked.insert(k.clone(), now);
    }

    /// Loads put in `cache::Shared` by tabs: the list learns of them, so
    /// their recordings count, and needn't be fetched again here.
    fn take_shared(&mut self, cx: &mut Context<Self>) {
        let Some(shared) = cache::shared(cx) else { return };
        let host = cache::host_key(&self.host);
        let new: Vec<(MrKey, Rc<Review>, u64, bool)> = shared
            .since(self.shared_seen)
            .filter(|(h, _)| *h == host)
            .map(|(_, e)| {
                let k = MrKey::new(cache::review_path(&e.review), e.review.mr.iid);
                (k, e.review.clone(), e.bytes, e.replayed)
            })
            .collect();
        self.shared_seen = shared.serial();
        if new.is_empty() {
            return;
        }
        let (now, secs) = (Instant::now(), now_secs());
        for (k, r, bytes, replayed) in new {
            if k.project.is_empty() {
                continue;
            }
            // A replay is what was recorded: it says nothing new, but a
            // list that forgot it learns its size.
            if replayed {
                if let Some(m) = self.mrs.get_mut(&k)
                    && m.review.is_none()
                {
                    m.review = Some(Recorded {
                        signature: Light::of_review(&r, None).signature(),
                        bytes,
                    });
                }
                continue;
            }
            // Ones it doesn't keep are taken when a tab shows them (`set_views`).
            if !self.mrs.contains_key(&k) && !self.open.contains(&k) {
                continue;
            }
            self.took(&k, &r, bytes, now, secs);
        }
        self.recompute_pinned();
        self.dirty = true;
        cx.notify();
    }

    fn failed_with(&mut self, e: Error) {
        if e != Error::Throttled {
            self.error = Some(e.to_string());
        }
    }

    fn upsert(&mut self, k: MrKey, l: model::Light, now: Instant, secs: u64) {
        let m = self.mrs.entry(k.clone()).or_insert_with(|| Mr::new(k.clone()));
        m.light = Some(l);
        m.gone = false;
        m.fetched_at = secs;
        self.checked.insert(k, now);
    }

    fn apply_checked(&mut self, c: api::Checked, had_lists: bool, now: Instant, secs: u64) {
        for (k, l) in c.mrs {
            match l {
                Some(l) => self.upsert(k, l, now, secs),
                None => {
                    if let Some(m) = self.mrs.get_mut(&k) {
                        m.gone = true;
                    }
                }
            }
        }
        if !had_lists {
            return;
        }
        let mut roles: HashMap<MrKey, model::Roles> = HashMap::new();
        for (list, set) in [
            (c.authored, 0),
            (c.review, 1),
            (c.assigned, 2),
        ] {
            for (k, l) in list.unwrap_or_default() {
                let r = roles.entry(k.clone()).or_default();
                match set {
                    0 => r.authored = true,
                    1 => r.review = true,
                    _ => r.assigned = true,
                }
                self.upsert(k, l, now, secs);
            }
        }
        for m in self.mrs.values_mut() {
            m.roles = roles.get(&m.key).copied().unwrap_or_default();
        }
        for (_, list) in c.branches {
            for (k, l) in list {
                self.upsert(k, l, now, secs);
            }
        }
        self.lists_ok = true;
        self.recompute_pinned();
    }

    /// Each workspace's merge requests: open ones from its branches.
    fn recompute_pinned(&mut self) {
        self.pinned = self
            .found
            .iter()
            .map(|(ws, projects)| {
                let keys = self
                    .mrs
                    .values()
                    .filter(|m| {
                        m.light.as_ref().is_some_and(|l| {
                            l.is_open()
                                && projects.iter().any(|(p, bs)| {
                                    *p == m.key.project && bs.contains(&l.source_branch)
                                })
                        })
                    })
                    .map(|m| m.key.clone())
                    .collect();
                (*ws, keys)
            })
            .collect();
    }

    fn evict(&mut self, cx: &mut Context<Self>) {
        if !self.found_once || !self.views_once {
            return;
        }
        let s = settings(cx).clone();
        let pinned: HashSet<MrKey> = self.pinned.values().flatten().cloned().collect();
        let dropped = model::evict(
            &mut self.mrs,
            &model::Keep {
                pinned: &pinned,
                open: &self.open,
                recent_limit: s.recent_limit as usize,
                cache_bytes: s.cache_size_mb as u64 * 1024 * 1024,
            },
        );
        let host = self.host.clone();
        for (k, project) in dropped {
            if let Some(p) = project {
                cache::forget(cx, &host, p, k.iid);
            }
        }
        let mrs = &self.mrs;
        self.checked.retain(|k, _| mrs.contains_key(k));
        if !self.pruned {
            self.pruned = true;
            self.prune(cx);
        }
    }

    /// Delete recordings of this host's merge requests the list doesn't keep
    /// (dropped while the app wasn't running, say).
    fn prune(&self, cx: &mut Context<Self>) {
        let host = cache::host_key(&self.host);
        let keep: HashSet<(u64, u64)> = self
            .mrs
            .values()
            .filter(|m| m.review.is_some())
            .filter_map(|m| Some((m.project_id()?, m.key.iid)))
            .collect();
        cx.background_executor()
            .spawn(async move {
                for (h, p, iid, _) in cache::saved() {
                    if h == host && !keep.contains(&(p, iid)) {
                        cache::forget_file(&h, p, iid);
                    }
                }
            })
            .detach();
    }

    // ---- what the app tells it ------------------------------------------

    /// Load it again first thing, whatever its schedule.
    pub fn refetch(&mut self, k: &MrKey) {
        self.failed.remove(k);
        self.mrs.entry(k.clone()).or_insert_with(|| Mr::new(k.clone()));
        if !self.urgent.contains(k) {
            self.urgent.push_back(k.clone());
        }
        self.wake();
    }

    /// Refresh in the panel: check everything now.
    pub fn refresh_all(&mut self, cx: &mut Context<Self>) {
        self.checked.clear();
        self.lists_checked = None;
        self.failed.clear();
        self.found_at = None;
        self.who_at = None;
        self.look_up_who(cx);
        self.wake();
        cx.notify();
    }

    /// The merge requests GitLab tabs show (`open`) and which of them are
    /// on screen. One newly shown is "recently opened".
    pub fn set_views(
        &mut self,
        open: HashSet<MrKey>,
        on_screen: HashSet<MrKey>,
        active: bool,
        cx: &mut Context<Self>,
    ) {
        let first = !self.views_once;
        self.views_once = true;
        if open == self.open && on_screen == self.on_screen && active == self.app_active && !first {
            return;
        }
        let secs = now_secs();
        let newly: Vec<MrKey> = open.difference(&self.open).cloned().collect();
        for k in &newly {
            let m = self.mrs.entry(k.clone()).or_insert_with(|| Mr::new(k.clone()));
            m.inspected_at = Some(secs);
            m.review_skip = None;
        }
        let back = active && !self.app_active;
        let new_on_screen = on_screen.iter().any(|k| !self.on_screen.contains(k));
        let closed = self.open.iter().any(|k| !open.contains(k));
        self.open = open;
        self.on_screen = on_screen;
        self.app_active = active;
        if back {
            // Coming back: anything older than half a minute is checked.
            let now = Instant::now();
            self.checked
                .retain(|_, t| now.duration_since(*t) < Duration::from_secs(30));
        }
        if !newly.is_empty() {
            // Their loads may be in already.
            self.shared_seen = 0;
            self.take_shared(cx);
        }
        if back || new_on_screen || !newly.is_empty() {
            self.wake();
        }
        if !newly.is_empty() || closed || first {
            self.evict(cx);
            self.dirty = true;
            cx.notify();
        }
    }

    /// The workspaces and where their checkouts are. Looking for their
    /// projects and branches runs in the background, when they change or
    /// once per `mineSeconds`.
    pub fn set_workspaces(&mut self, places: HashMap<usize, Vec<Place>>, cx: &mut Context<Self>) {
        let s = settings(cx).clone();
        let stale = self
            .found_at
            .is_none_or(|t| t.elapsed() >= Duration::from_secs(s.mine_seconds.max(15) as u64));
        if self.finding || self.host.is_empty() || (places == self.places && !stale) {
            return;
        }
        self.places = places.clone();
        self.finding = true;
        let host = remote::host_name(&self.host);
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move {
                    places
                        .into_iter()
                        .map(|(ws, p)| (ws, remote::discover(&p, &host)))
                        .collect::<HashMap<_, _>>()
                })
                .await;
            let _ = this.update(cx, |s, cx| {
                s.finding = false;
                s.found_at = Some(Instant::now());
                let first = !s.found_once;
                s.found_once = true;
                if s.found != found {
                    s.found = found;
                    // The new branches are asked about at once.
                    s.lists_checked = None;
                    s.wake();
                }
                s.recompute_pinned();
                if first {
                    s.wake();
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ---- for the panel ----------------------------------------------------

    /// Its load is under way or waiting.
    pub fn refreshing(&self, k: &MrKey) -> bool {
        self.urgent.contains(k) || self.busy.contains(k)
    }

    /// While paused for a 429: how long is left.
    pub fn paused(&self) -> Option<Duration> {
        self.throttle.lock().unwrap().paused(Instant::now())
    }

    pub fn slowdown(&self) -> f64 {
        self.throttle.lock().unwrap().slow
    }

    /// Mine, newest update first.
    pub fn mine(&self) -> Vec<&Mr> {
        let mut v: Vec<&Mr> = self.mrs.values().filter(|m| m.roles.any()).collect();
        v.sort_by(|a, b| updated(b).cmp(updated(a)));
        v
    }

    /// Recently opened, newest first.
    pub fn recent(&self) -> Vec<&Mr> {
        let mut v: Vec<&Mr> = self.mrs.values().filter(|m| m.inspected_at.is_some()).collect();
        v.sort_by_key(|m| std::cmp::Reverse(m.inspected_at));
        v
    }

    /// Its page on GitLab.
    pub fn web_url(&self, k: &MrKey) -> String {
        self.mrs
            .get(k)
            .and_then(|m| m.light.as_ref())
            .map(|l| l.web_url.clone())
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| format!("{}/{}/-/merge_requests/{}", self.host, k.project, k.iid))
    }
}

fn updated(m: &Mr) -> &str {
    m.light.as_ref().map_or("", |l| l.updated_at.as_str())
}

// ---- dates --------------------------------------------------------------

/// Seconds since the epoch of GitLab's `2026-11-06T10:20:30.123Z` (or
/// `+02:00` at the end).
pub fn parse_time(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    // Days from civil (Howard Hinnant).
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * (mo + if mo > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    let mut t = days * 86400 + h * 3600 + mi * 60 + sec;
    let tail = &s[19..];
    let tz = tail.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    if let Some(off) = tz.strip_prefix('+').or_else(|| tz.strip_prefix('-')) {
        let sign = if tz.starts_with('+') { 1 } else { -1 };
        let oh = off.get(0..2)?.parse::<i64>().ok()?;
        let om = off.get(3..5).and_then(|m| m.parse::<i64>().ok()).unwrap_or(0);
        t -= sign * (oh * 3600 + om * 60);
    }
    Some(t)
}

/// "just now", "5m ago", "3h ago", "2d ago".
pub fn ago(secs: i64) -> String {
    let d = (now_secs() as i64 - secs).max(0);
    match d {
        0..60 => "just now".into(),
        60..3600 => format!("{}m ago", d / 60),
        3600..86400 => format!("{}h ago", d / 3600),
        _ => format!("{}d ago", d / 86400),
    }
}

pub fn ago_iso(s: &str) -> String {
    parse_time(s).map(ago).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_times() {
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_time("2026-11-06T10:20:30.123Z"), Some(1793960430));
        assert_eq!(parse_time("2026-11-06T12:20:30+02:00"), Some(1793960430));
        assert_eq!(parse_time("nope"), None);
    }
}
