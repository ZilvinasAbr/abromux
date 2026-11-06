//! The GitLab tab: sign in, the merge request list, and one merge request
//! (mr.rs: header, Overview, Commits; changes.rs: Changes).
//!
//! Network calls run on the background executor; each page holds a [`Load`]
//! and redraws when it lands. Everything the tab shows is also in
//! [`GitLabView::state`], and [`GitLabView::command`] does what a click does,
//! so scripts and tests drive it over `abromux rpc surface.plugin`.

use crate::{
    Compare, ListScope, Location, MrState, Section,
    api::{self, Client, Curl, Scope},
    auth,
    avatars::Avatars,
    changes::{FileText, Picture, Row},
    load_review,
    review::Review,
    style::*,
    time,
    types::*,
};
use abromux_plugin::Tab;
use anyhow::{Context as _, bail};
use gpui::{prelude::*, *};
use gpui_component::input::{Input, InputEvent, InputState};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::Arc,
};

pub enum Load<T> {
    Idle,
    Loading,
    Ready(T),
    Failed(String),
}

impl<T> Load<T> {
    fn name(&self) -> &'static str {
        match self {
            Load::Idle => "idle",
            Load::Loading => "loading",
            Load::Ready(_) => "ready",
            Load::Failed(_) => "failed",
        }
    }

    fn error(&self) -> Option<&str> {
        match self {
            Load::Failed(e) => Some(e),
            _ => None,
        }
    }

    pub fn ready(&self) -> Option<&T> {
        match self {
            Load::Ready(t) => Some(t),
            _ => None,
        }
    }
}

/// Where the merge request to open is.
enum Target {
    Id(u64, u64),
    Path(String, u64),
}

pub struct GitLabView {
    pub(crate) loc: Location,
    focus: FocusHandle,
    pub(crate) client: Option<Client>,
    pub(crate) me: Option<User>,
    list: Load<Vec<MergeRequest>>,
    /// Listed merge requests that are approved, by id.
    approved: HashSet<u64>,
    /// Listed merge requests' (resolved, resolvable) threads, by id.
    list_threads: HashMap<u64, (usize, usize)>,
    /// Listed merge requests' latest pipelines, by id.
    list_pipelines: HashMap<u64, Pipeline>,
    /// Listed projects' default branches, by project id.
    default_branches: HashMap<u64, String>,
    pub(crate) review: Load<Rc<Review>>,
    pub(crate) avatars: Avatars,
    /// Threads whose replies are folded.
    pub(crate) collapsed_replies: HashSet<String>,
    /// Resolved threads shown open.
    pub(crate) open_resolved: HashSet<String>,
    /// Files folded or unfolded by hand (else: folded when viewed).
    pub(crate) folded: HashMap<String, bool>,
    /// The file tree shown or hidden by hand; None: when it fits.
    pub(crate) show_tree: Option<bool>,
    /// The file browser as a flat list of paths (GitLab's list mode).
    pub(crate) tree_flat: bool,
    /// Folders folded in the file tree, by path.
    pub(crate) folded_dirs: HashSet<String>,
    /// The file tree's scroll, and the file it last scrolled to.
    pub(crate) tree_scroll: ScrollHandle,
    pub(crate) tree_active: std::cell::Cell<Option<usize>>,
    /// The line whose number was clicked: (file, old line, new line).
    pub(crate) selected_line: Option<(usize, Option<u32>, Option<u32>)>,
    /// GitLab's "Show one file at a time": the file shown, by index.
    pub(crate) single_file: Option<usize>,
    /// Commits whose message body is shown, by sha.
    pub(crate) open_bodies: HashSet<String>,
    /// Files' whole text, fetched when expanding unchanged lines.
    pub(crate) contents: HashMap<usize, Load<Rc<FileText>>>,
    /// Image previews of binary files.
    pub(crate) pictures: HashMap<usize, Load<Picture>>,
    /// Unchanged lines shown above (file, hunk), and after a file's last hunk.
    pub(crate) above: HashMap<(usize, usize), u32>,
    pub(crate) below: HashMap<usize, u32>,
    /// The row of the thread `n`/`p` last went to.
    pub(crate) thread_cursor: Option<usize>,
    /// The Changes tab's rows, and its list.
    pub(crate) rows: Vec<Row>,
    pub(crate) diff_list: ListState,
    /// The Overview tab's list (see mr.rs).
    pub(crate) overview_list: ListState,
    /// The project path of a merge request opened by URL, until it loads
    /// and its project id is known.
    pending_path: Option<String>,
    /// The tab's width at the last frame: in a split pane it's much less
    /// than the window's, and the layout narrows as GitLab's does.
    pub(crate) width: Rc<std::cell::Cell<Pixels>>,
    /// The version picker's choice, and its diffs when not the latest.
    pub(crate) compare: Compare,
    compared: Load<Rc<Review>>,
    /// Which of the picker's menus is open: the base (left) or the head.
    pub(crate) menu: Option<bool>,
    /// The overview's activity: which entries, which order, the menu open.
    pub(crate) activity: crate::mr::ActivityFilter,
    pub(crate) newest_first: bool,
    pub(crate) activity_menu: bool,
    /// Writing (edit.rs): the text fields, a write under way, the last
    /// one's error, the thread being replied to, the line being commented on.
    pub(crate) boxes: crate::edit::Boxes,
    pub(crate) busy: bool,
    pub(crate) write_error: Option<String>,
    pub(crate) replying: Option<String>,
    pub(crate) commenting: Option<crate::edit::LineKey>,
    /// The diff line under the pointer: the one with the comment bubble.
    pub(crate) hovered_line: Option<crate::edit::LineKey>,
    /// The file browser's search field.
    pub(crate) tree_search: Entity<InputState>,
    /// The merge request list's filter field.
    list_search: Entity<InputState>,
    /// No token for the host: the sign-in form's host and token fields.
    sign_in: Option<(Entity<InputState>, Entity<InputState>)>,
    sign_in_error: Option<String>,
    _list_task: Option<Task<()>>,
    _review_task: Option<Task<()>>,
    _sign_in_task: Option<Task<()>>,
    _poll: Task<()>,
}

impl GitLabView {
    pub fn new(mut loc: Location, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // A new tab: the GitLab signed in to before, unless a dev token is set.
        if loc.host.is_none() && std::env::var_os("ABROMUX_GITLAB_TOKEN").is_none() {
            loc.host = auth::saved_host();
        }
        let tree_search = cx.new(|cx| InputState::new(window, cx).placeholder("Search (e.g. *.rs)"));
        cx.subscribe(&tree_search, |_, _, _: &InputEvent, cx| cx.notify()).detach();
        let boxes = crate::edit::Boxes::new(window, cx);
        for b in [&boxes.comment, &boxes.reply, &boxes.line] {
            cx.subscribe(b, |_, _, _: &InputEvent, cx| cx.notify()).detach();
        }
        let list_search = cx.new(|cx| InputState::new(window, cx).placeholder("Search or filter results…"));
        cx.subscribe(&list_search, |_, _, _: &InputEvent, cx| cx.notify()).detach();
        let mut v = Self {
            boxes,
            busy: false,
            write_error: None,
            replying: None,
            commenting: None,
            hovered_line: None,
            list_search,
            pending_path: None,
            width: Rc::new(std::cell::Cell::new(px(1600.))),
            tree_search,
            compare: Compare::default(),
            compared: Load::Idle,
            menu: None,
            activity: Default::default(),
            newest_first: false,
            activity_menu: false,
            loc,
            focus: cx.focus_handle(),
            client: None,
            me: None,
            list: Load::Idle,
            approved: HashSet::new(),
            default_branches: HashMap::new(),
            list_threads: HashMap::new(),
            list_pipelines: HashMap::new(),
            review: Load::Idle,
            avatars: Avatars::default(),
            collapsed_replies: HashSet::new(),
            open_resolved: HashSet::new(),
            folded: HashMap::new(),
            show_tree: None,
            tree_flat: false,
            folded_dirs: HashSet::new(),
            tree_scroll: ScrollHandle::new(),
            tree_active: Default::default(),
            selected_line: None,
            single_file: None,
            open_bodies: HashSet::new(),
            contents: HashMap::new(),
            pictures: HashMap::new(),
            above: HashMap::new(),
            below: HashMap::new(),
            thread_cursor: None,
            rows: vec![],
            diff_list: ListState::new(0, ListAlignment::Top, px(200.)),
            overview_list: ListState::new(0, ListAlignment::Top, px(200.)),
            sign_in: None,
            sign_in_error: None,
            _list_task: None,
            _review_task: None,
            _sign_in_task: None,
            _poll: Self::poll(cx),
        };
        v.connect(window, cx);
        v
    }

    fn host(&self) -> String {
        self.loc.host().to_string()
    }

    /// Use the stored token for the host, or show the sign-in form.
    fn connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let host = self.host();
        match auth::token(&host) {
            Some(token) => {
                self.client = Some(Client::new(&host, &token, Arc::new(Curl)));
                self.sign_in = None;
                self.load_list(cx);
                if let (Some(p), Some(iid)) = (self.loc.project, self.loc.iid) {
                    self.load_review(Target::Id(p, iid), cx);
                }
            }
            None => self.show_sign_in(window, cx),
        }
    }

    /// The sign-in form, for the tab's host.
    fn show_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.client = None;
        // No GitLab yet (and no dev token): an empty field, not localhost.
        let host = match &self.loc.host {
            None if std::env::var_os("ABROMUX_GITLAB_TOKEN").is_none() => String::new(),
            _ => self.host(),
        };
        let host_field =
            cx.new(|cx| InputState::new(window, cx).placeholder("https://gitlab.example.com").default_value(host));
        let token_field = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("Personal access token (api)")
        });
        self.sign_in = Some((host_field, token_field));
        cx.notify();
    }

    /// Check `token` on `host`, keep it in the Keychain, and use it.
    fn sign_in(&mut self, host: String, token: String, cx: &mut Context<Self>) {
        let host = host.trim().trim_end_matches('/').to_string();
        let token = token.trim().to_string();
        if host.is_empty() || token.is_empty() {
            self.sign_in_error = Some("Enter the GitLab URL and a token.".into());
            cx.notify();
            return;
        }
        self.sign_in_error = None;
        self._sign_in_task = Some(cx.spawn(async move |this, cx| {
            let (h, t) = (host.clone(), token.clone());
            let checked = cx
                .background_spawn(async move {
                    let user = Client::new(&h, &t, Arc::new(Curl)).current_user()?;
                    auth::save_token(&h, &t)?;
                    anyhow::Ok(user)
                })
                .await;
            this.update(cx, |v, cx| {
                match checked {
                    Ok(user) => {
                        v.loc.host = Some(host.clone());
                        v.me = Some(user);
                        v.client = Some(Client::new(&host, &token, Arc::new(Curl)));
                        v.sign_in = None;
                        v.load_list(cx);
                        // Back to the merge request the tab was on.
                        match (v.loc.project, v.loc.iid, v.pending_path.clone()) {
                            (Some(p), Some(iid), _) => v.load_review(Target::Id(p, iid), cx),
                            (None, Some(iid), Some(path)) => v.load_review(Target::Path(path, iid), cx),
                            _ => {}
                        }
                    }
                    Err(e) => v.sign_in_error = Some(format!("{e:#}")),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn load_list(&mut self, cx: &mut Context<Self>) {
        let Some(c) = self.client.clone() else { return };
        let (scope, me, state) = (self.loc.scope, self.me.clone(), self.loc.state);
        self.list = Load::Loading;
        self._list_task = Some(cx.spawn(async move |this, cx| {
            let got = cx
                .background_spawn(async move {
                    let me = match me {
                        Some(me) => me,
                        None => c.current_user()?,
                    };
                    let scope = match scope {
                        ListScope::ToReview => Scope::ToReview(me.username.clone()),
                        ListScope::Mine => Scope::Mine,
                        ListScope::All => Scope::All,
                    };
                    let mrs = c.merge_requests_in(scope, state.api())?;
                    // The list's "Approved" badges: one request each, so only the first rows.
                    let approved: HashSet<u64> = mrs
                        .iter()
                        .take(50)
                        .filter(|m| c.approvals(m.project_id, m.iid).is_ok_and(|a| a.approved && !a.approved_by.is_empty()))
                        .map(|m| m.id)
                        .collect();
                    // Each project's default branch, to name other targets as GitLab does.
                    let mut defaults: HashMap<u64, String> = HashMap::new();
                    for m in mrs.iter().take(50) {
                        defaults
                            .entry(m.project_id)
                            .or_insert_with(|| c.default_branch(m.project_id).unwrap_or_default());
                    }
                    // Resolved threads ("1 of 4"), as GitLab's list shows them.
                    let threads: HashMap<u64, (usize, usize)> = mrs
                        .iter()
                        .take(50)
                        .filter(|m| m.user_notes_count > 0)
                        .filter_map(|m| {
                            let ds = c.discussions(m.project_id, m.iid).ok()?;
                            let resolvable: Vec<_> = ds.iter().filter(|d| d.resolvable()).collect();
                            let resolved = resolvable.iter().filter(|d| d.resolved()).count();
                            Some((m.id, (resolved, resolvable.len())))
                        })
                        .collect();
                    let pipelines: HashMap<u64, Pipeline> = mrs
                        .iter()
                        .take(50)
                        .filter_map(|m| Some((m.id, c.latest_pipeline(m.project_id, m.iid).ok()??)))
                        .collect();
                    anyhow::Ok((me, mrs, approved, defaults, threads, pipelines))
                })
                .await;
            this.update(cx, |v, cx| {
                v.list = match got {
                    Ok((me, mrs, approved, defaults, threads, pipelines)) => {
                        v.list_pipelines = pipelines;
                        v.list_threads = threads;
                        v.me = Some(me);
                        v.approved = approved;
                        v.default_branches = defaults;
                        let people = mrs.iter().flat_map(|m| m.assignees.iter().chain(&m.reviewers));
                        v.avatars.want(people.collect::<Vec<_>>(), cx, |v| &mut v.avatars);
                        v.avatars.want(mrs.iter().map(|m| &m.author), cx, |v| &mut v.avatars);
                        Load::Ready(mrs)
                    }
                    Err(e) => Load::Failed(format!("{e:#}")),
                };
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn load_review(&mut self, target: Target, cx: &mut Context<Self>) {
        self.fetch_review(target, false, cx);
    }

    /// Load a merge request. `quiet`: the one showing, again; it stays on
    /// screen until the new one is in (and if that fails), and where you
    /// are in it is kept, as GitLab updates its page while you read.
    fn fetch_review(&mut self, target: Target, quiet: bool, cx: &mut Context<Self>) {
        let Some(c) = self.client.clone() else { return };
        let quiet = quiet && self.review.ready().is_some();
        if !quiet {
            self.review = Load::Loading;
        }
        self._review_task = Some(cx.spawn(async move |this, cx| {
            let got = cx
                .background_spawn(async move {
                    let (p, iid) = match target {
                        Target::Id(p, iid) => (p, iid),
                        Target::Path(path, iid) => (c.project_id(&path)?, iid),
                    };
                    load_review(&c, p, iid)
                })
                .await;
            this.update(cx, |v, cx| {
                match got {
                    Ok(r) => {
                        v.loc.project = Some(r.mr.project_id);
                        v.loc.iid = Some(r.mr.iid);
                        v.pending_path = None;
                        v.want_avatars(&r, cx);
                        // Fetched texts, images and expansions are by file
                        // index: only good while the files are the same.
                        let same_files = v.review.ready().is_some_and(|old| {
                            old.files.iter().map(|f| f.path()).eq(r.files.iter().map(|f| f.path()))
                                && old.mr.diff_refs == r.mr.diff_refs
                        });
                        if !same_files {
                            v.contents.clear();
                            v.selected_line = None;
                            v.pictures.clear();
                            v.above.clear();
                            v.below.clear();
                        }
                        v.review = Load::Ready(Rc::new(r));
                    }
                    // A quiet refresh that fails keeps what's showing.
                    Err(_) if quiet => return,
                    Err(e) => v.review = Load::Failed(format!("{e:#}")),
                }
                v.rebuild_rows(cx);
                v.fetch_pictures(cx);
            })
            .ok();
        }));
        cx.notify();
    }

    fn want_avatars(&mut self, r: &Review, cx: &mut Context<Self>) {
        let mut users: Vec<&User> = vec![&r.mr.author];
        users.extend(&r.mr.assignees);
        users.extend(&r.mr.reviewers);
        users.extend(r.approvals.approved_by.iter().map(|a| &a.user));
        let threads = r.general.iter().chain(&r.outdated).chain(r.files.iter().flat_map(|f| f.threads.iter().map(|(_, _, d)| d).chain(&f.outdated)));
        users.extend(threads.flat_map(|d| d.notes.iter().map(|n| &n.author)));
        users.extend(r.system.iter().map(|n| &n.author));
        self.avatars.want(users, cx, |v| &mut v.avatars);
    }

    fn open_mr(&mut self, project: u64, iid: u64, cx: &mut Context<Self>) {
        if self.loc.iid != Some(iid) || self.loc.project != Some(project) {
            self.loc.viewed.clear();
            self.folded.clear();
            self.open_resolved.clear();
            self.collapsed_replies.clear();
        }
        // Fetched texts and expansions are of the version being loaded again.
        self.compare = Compare::default();
        self.compared = Load::Idle;
        self.contents.clear();
        self.selected_line = None;
        self.pictures.clear();
        self.above.clear();
        self.below.clear();
        self.loc.project = Some(project);
        self.loc.iid = Some(iid);
        self.loc.section = Section::Overview;
        self.load_review(Target::Id(project, iid), cx);
    }

    /// A merge request's web URL; another host than this tab's switches it.
    fn open_url(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) -> anyhow::Result<()> {
        let (host, path, iid) = api::parse_mr_url(url).context("not a merge request URL")?;
        if host != self.host() {
            self.loc.host = Some(host);
            self.me = None;
            self.connect(window, cx);
        }
        if self.client.is_none() {
            bail!("no token for {}: sign in first", self.host());
        }
        self.loc.iid = Some(iid);
        self.loc.section = if url.contains("/diffs") { Section::Changes } else { Section::Overview };
        self.pending_path = Some(path.clone());
        self.load_review(Target::Path(path, iid), cx);
        Ok(())
    }

    fn back(&mut self, cx: &mut Context<Self>) {
        self.loc.project = None;
        self.loc.iid = None;
        self.loc.viewed.clear();
        self.review = Load::Idle;
        self._review_task = None;
        self.rows.clear();
        self.diff_list.reset(0);
        if matches!(self.list, Load::Idle | Load::Failed(_)) {
            self.load_list(cx);
        }
        cx.notify();
    }

    fn set_scope(&mut self, scope: ListScope, cx: &mut Context<Self>) {
        self.loc.scope = scope;
        self.load_list(cx);
    }

    fn set_state(&mut self, state: MrState, cx: &mut Context<Self>) {
        self.loc.state = state;
        self.load_list(cx);
    }

    pub(crate) fn set_section(&mut self, s: Section, cx: &mut Context<Self>) {
        self.loc.section = s;
        cx.notify();
    }

    /// The next or previous unresolved thread, in the Changes tab.
    pub(crate) fn jump_thread(&mut self, forward: bool, cx: &mut Context<Self>) {
        self.loc.section = Section::Changes;
        self.step_thread(forward, cx);
    }

    /// GitLab's review shortcuts: `j`/`]` and `k`/`[` step through files,
    /// `n` and `p` through unresolved threads.
    fn key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        // Typing in a comment: the keys are the text's.
        if self.boxes.typing(window, cx) {
            if k.key == "escape" {
                self.replying = None;
                self.commenting = None;
                window.focus(&self.focus);
                self.rebuild_rows(cx);
            }
            return;
        }
        // Escape closes an open menu first, as in GitLab.
        if k.key == "escape" && (self.menu.is_some() || self.activity_menu) {
            self.menu = None;
            self.activity_menu = false;
            cx.notify();
            cx.stop_propagation();
            return;
        }
        if !self.in_mr() || k.modifiers.modified() {
            return;
        }
        match k.key.as_str() {
            "j" | "]" => {
                self.loc.section = Section::Changes;
                self.step_file(true, cx)
            }
            "k" | "[" => {
                self.loc.section = Section::Changes;
                self.step_file(false, cx)
            }
            "n" => self.jump_thread(true, cx),
            "p" => self.jump_thread(false, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    /// The open merge request again, in place (after a write).
    pub(crate) fn refresh_quietly(&mut self, cx: &mut Context<Self>) {
        if let (Some(p), Some(iid)) = (self.loc.project, self.loc.iid) {
            self.fetch_review(Target::Id(p, iid), true, cx);
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        match (self.loc.project, self.loc.iid) {
            (Some(p), Some(iid)) => self.fetch_review(Target::Id(p, iid), true, cx),
            _ => self.load_list(cx),
        }
    }

    /// Every two minutes, the merge request showing is fetched again,
    /// quietly: new comments and pushes appear as on GitLab's page.
    fn poll(cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_secs(120)).await;
                let alive = this.update(cx, |v, cx| {
                    if let (Some(p), Some(iid), Some(_)) = (v.loc.project, v.loc.iid, v.review.ready()) {
                        v.fetch_review(Target::Id(p, iid), true, cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
    }

    fn in_mr(&self) -> bool {
        self.loc.iid.is_some()
    }

    /// GitLab's own page for what the tab shows.
    fn web_url(&self) -> String {
        match (self.review.ready(), self.in_mr()) {
            (Some(r), true) if !r.mr.web_url.is_empty() => match self.loc.section {
                Section::Overview => r.mr.web_url.clone(),
                Section::Commits => format!("{}/commits", r.mr.web_url),
                Section::Changes => format!("{}/diffs", r.mr.web_url),
            },
            _ => format!("{}/dashboard/merge_requests", self.host()),
        }
    }

    /// Whether the file tree is showing: as toggled, else when it fits
    /// beside the diff.
    pub(crate) fn tree_shown(&self) -> bool {
        self.show_tree.unwrap_or(self.width.get() >= px(900.))
    }

    /// Wide enough for the Overview's sidebar.
    pub(crate) fn sidebar_fits(&self) -> bool {
        self.width.get() >= px(1000.)
    }

    /// The review the Changes tab shows: the latest version's, or the
    /// picked comparison's once it's loaded.
    pub(crate) fn shown(&self) -> Option<Rc<Review>> {
        if self.compare == Compare::default() {
            self.review.ready().cloned()
        } else {
            self.compared.ready().cloned()
        }
    }

    /// Pick other versions to compare; their diffs load in the background.
    pub(crate) fn set_compare(&mut self, compare: Compare, cx: &mut Context<Self>) {
        self.menu = None;
        if compare == self.compare {
            cx.notify();
            return;
        }
        self.compare = compare;
        // These are by file index, which another set of diffs changes.
        self.contents.clear();
        self.selected_line = None;
        self.pictures.clear();
        self.above.clear();
        self.below.clear();
        if compare == Compare::default() {
            self.compared = Load::Idle;
            self.rebuild_rows(cx);
            self.fetch_pictures(cx);
            return;
        }
        let (Some(r), Some(c)) = (self.review.ready().map(|r| Review::clone(r)), self.client.clone()) else { return };
        self.compared = Load::Loading;
        self.rebuild_rows(cx);
        cx.spawn(async move |this, cx| {
            let got = cx.background_spawn(async move { crate::load_compare(&c, &r, compare) }).await;
            this.update(cx, |v, cx| {
                // Only if it's still the one asked for.
                if v.compare == compare {
                    v.compared = got.map_or_else(|e| Load::Failed(format!("{e:#}")), |r| Load::Ready(Rc::new(r)));
                    v.rebuild_rows(cx);
                    v.fetch_pictures(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn compare_loading(&self) -> Option<Option<&str>> {
        if self.compare == Compare::default() {
            return None;
        }
        match &self.compared {
            Load::Ready(_) => None,
            Load::Failed(e) => Some(Some(e)),
            _ => Some(None),
        }
    }

    /// A thread's id, by the start of its first note's body.
    fn thread_by_body(&self, text: &str) -> anyhow::Result<String> {
        let r = self.review.ready().context("no merge request loaded")?;
        let all = r.general.iter().chain(&r.outdated).chain(r.files.iter().flat_map(|f| f.threads.iter().map(|(_, _, d)| d)));
        Ok(all.into_iter().find(|d| d.notes[0].body.starts_with(text)).context("no such thread")?.id.clone())
    }

    fn file_index(&self, path: &str) -> anyhow::Result<usize> {
        let r = self.shown().context("no merge request loaded")?;
        r.files.iter().position(|f| f.path() == path).with_context(|| format!("no file {path}"))
    }
}

impl Focusable for GitLabView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Tab for GitLabView {
    fn title(&self, _: &App) -> String {
        match (&self.review, self.loc.iid) {
            (Load::Ready(r), Some(_)) => format!("!{} {}", r.mr.iid, r.mr.title),
            (_, Some(iid)) => format!("!{iid}"),
            _ => "Merge Requests".into(),
        }
    }

    fn icon(&self) -> &'static str {
        "git-pull-request"
    }

    fn save(&self, _: &App) -> Value {
        serde_json::to_value(&self.loc).unwrap_or_default()
    }

    fn state(&self, cx: &App) -> Value {
        let page = if self.sign_in.is_some() {
            "sign_in"
        } else if self.in_mr() {
            "merge_request"
        } else {
            "list"
        };
        let list = self.list.ready().map(|mrs| {
            mrs.iter()
                .map(|m| json!({ "ref": m.references.full, "title": m.title, "draft": m.draft }))
                .collect::<Vec<_>>()
        });
        let r = self.review.ready();
        // The Changes tab's files: the picked versions' when comparing.
        let shown = self.shown();
        let folded: Vec<&str> = shown
            .as_deref()
            .map(|r| r.files.iter().filter(|f| self.is_folded(f)).map(|f| f.path()).collect())
            .unwrap_or_default();
        json!({
            "location": self.save(cx),
            "host": self.host(),
            "page": page,
            "me": self.me.as_ref().map(|u| &u.username),
            "sign_in_error": self.sign_in_error,
            "list": { "load": self.list.name(), "error": self.list.error(), "items": list },
            "review": {
                "load": self.review.name(),
                "error": self.review.error(),
                "summary": r.map(|r| r.summary()),
            },
            "changes": {
                "rows": self.rows.len(),
                "threads_shown": self.rows.iter().filter(|r| matches!(r, Row::Thread(..))).count(),
                "top_row": self.diff_list.logical_scroll_top().item_ix,
                "active_file": self.active_file().and_then(|i| shown.as_ref().map(|r| r.files[i].path().to_string())),
                "files": shown.as_ref().map(|r| r.files.len()),
                "compare": { "base": self.compare.base, "head": self.compare.head, "commit": self.compare.commit },
                "folded": folded,
                "tree": self.tree_shown(),
                "width": f32::from(self.width.get()),
            },
            "edit": {
                "busy": self.busy,
                "error": self.write_error,
                "replying": self.replying,
                "commenting": self.commenting,
            },
        })
    }

    fn command(
        &mut self,
        name: &str,
        args: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<Value> {
        let str_arg = |k: &str| args.get(k).and_then(Value::as_str);
        let num_arg = |k: &str| args.get(k).and_then(Value::as_u64);
        match name {
            "state" => {}
            "refresh" => self.refresh(cx),
            "back" => self.back(cx),
            "list" => {
                let scope = serde_json::from_value(json!(str_arg("scope").unwrap_or("to_review")))
                    .context("scope is to_review, mine or all")?;
                let state = serde_json::from_value(json!(str_arg("state").unwrap_or("opened")))
                    .context("state is opened, merged, closed or all")?;
                self.back(cx);
                self.loc.state = state;
                self.set_scope(scope, cx);
            }
            "open" => match (str_arg("url"), num_arg("project"), num_arg("iid")) {
                (Some(url), _, _) => self.open_url(url, window, cx)?,
                (None, Some(p), Some(iid)) => self.open_mr(p, iid, cx),
                (None, None, Some(iid)) => {
                    let mrs = self.list.ready().context("the list isn't loaded")?;
                    let m = mrs.iter().find(|m| m.iid == iid).context("not in the list")?;
                    let p = m.project_id;
                    self.open_mr(p, iid, cx);
                }
                _ => bail!("open takes url, or iid (and project)"),
            },
            "section" => {
                let s = serde_json::from_value(json!(str_arg("name").unwrap_or("")))
                    .context("section is overview, changes or commits")?;
                self.set_section(s, cx);
            }
            // The Changes tab: scroll to, fold, mark viewed.
            "file" => {
                let i = self.file_index(str_arg("path").context("missing path")?)?;
                self.scroll_to_file(i, cx);
            }
            "fold" => self.toggle_folded(str_arg("path").context("missing path")?, cx),
            "viewed" => {
                let p = str_arg("path").context("missing path")?;
                self.file_index(p)?;
                self.toggle_viewed(p, cx);
            }
            "tree" => self.show_tree = Some(!self.tree_shown()),
            "one_file" => {
                let on = args.get("on").and_then(Value::as_bool).unwrap_or(true);
                self.set_single(on, cx);
            }
            "tree_list" => self.tree_flat = args.get("on").and_then(Value::as_bool).unwrap_or(true),
            "parallel" => self.set_parallel(args.get("on").and_then(Value::as_bool).unwrap_or(true), cx),
            // The file browser's search, or on the list page the list's filter.
            "search" => {
                let q = str_arg("query").unwrap_or_default().to_string();
                let field = if self.in_mr() { &self.tree_search } else { &self.list_search };
                field.update(cx, |s, cx| s.set_value(q, window, cx));
            }
            // The version picker, by GitLab's numbers: {"head": 1} is version 1
            // against the target branch, {"base": 1, "head": 2} one against
            // another, {} the latest.
            "compare" => {
                let r = self.review.ready().context("no merge request loaded")?.clone();
                let id = |k: &str| -> anyhow::Result<Option<u64>> {
                    let Some(n) = num_arg(k) else { return Ok(None) };
                    let i = r.versions.len().checked_sub(n as usize).context("no such version")?;
                    Ok(Some(r.versions.get(i).context("no such version")?.id))
                };
                let latest = r.versions.first().map(|v| v.id);
                let head = id("head")?.filter(|h| Some(*h) != latest);
                let base = id("base")?;
                let head = if base.is_some() { head.or(latest) } else { head };
                self.set_compare(Compare { base, head, commit: None }, cx);
            }
            // The overview's activity: {"filter": "all"|"comments"|"history", "newest_first": bool}.
            "activity" => {
                use crate::mr::ActivityFilter::*;
                if let Some(f) = str_arg("filter") {
                    self.activity = match f {
                        "comments" => Comments,
                        "history" => History,
                        _ => All,
                    };
                }
                if let Some(n) = args.get("newest_first").and_then(Value::as_bool) {
                    self.newest_first = n;
                }
            }
            // One commit's changes, by its place in the Commits tab (0: newest) or sha.
            "commit" => {
                let r = self.review.ready().context("no merge request loaded")?.clone();
                let i = match (num_arg("index"), str_arg("sha")) {
                    (Some(i), _) => i as usize,
                    (None, Some(sha)) => r.commits.iter().position(|c| c.id.starts_with(sha)).context("no such commit")?,
                    _ => bail!("commit takes index or sha"),
                };
                anyhow::ensure!(i < r.commits.len(), "no commit {i}");
                self.open_commit(i, cx);
            }
            "next_thread" => self.jump_thread(true, cx),
            "prev_thread" => self.jump_thread(false, cx),
            // Show unchanged lines: above hunk `hunk` (else after the last), `lines` of them (else all).
            "expand" => {
                let fi = self.file_index(str_arg("path").context("missing path")?)?;
                let hunk = num_arg("hunk").map(|h| h as usize);
                let n = num_arg("lines").map_or(u32::MAX, |n| n as u32);
                self.expand(fi, hunk, n, cx);
            }
            // From the overview to a thread on its line, by the first note's body start.
            "show_thread" => {
                let r = self.review.ready().context("no merge request loaded")?.clone();
                let text = str_arg("body").context("missing body")?;
                let id = r
                    .files
                    .iter()
                    .flat_map(|f| f.threads.iter().map(|(_, _, d)| d))
                    .find(|d| d.notes[0].body.starts_with(text))
                    .context("no such thread on a line")?
                    .id
                    .clone();
                self.show_thread(&id, cx);
            }
            // Open or fold a resolved thread, by the first note's body start.
            "thread" => {
                let r = self.review.ready().context("no merge request loaded")?.clone();
                let text = str_arg("body").context("missing body")?;
                let all = r.general.iter().chain(&r.outdated).chain(r.files.iter().flat_map(|f| f.threads.iter().map(|(_, _, d)| d)));
                let d = all.into_iter().find(|d| d.notes[0].body.starts_with(text)).context("no such thread")?;
                if !self.open_resolved.remove(&d.id) {
                    self.open_resolved.insert(d.id.clone());
                }
                self.rebuild_rows(cx);
            }
            // Writing, as the buttons do: fill the box, then press it.
            "comment" => {
                let body = str_arg("body").context("missing body")?.to_string();
                self.boxes.comment.update(cx, |b, cx| b.set_value(body, window, cx));
                if args.get("send").and_then(Value::as_bool) != Some(false) {
                    self.post_comment(window, cx);
                }
            }
            "reply" => {
                let id = self.thread_by_body(str_arg("thread").context("missing thread")?)?;
                self.open_reply(&id, window, cx);
                if let Some(body) = str_arg("body") {
                    self.boxes.reply.update(cx, |b, cx| b.set_value(body.to_string(), window, cx));
                    if args.get("send").and_then(Value::as_bool) != Some(false) {
                        let resolve = args.get("resolve").and_then(Value::as_bool) == Some(true);
                        self.send_reply(resolve, window, cx);
                    }
                }
            }
            "resolve" => {
                let id = self.thread_by_body(str_arg("thread").context("missing thread")?)?;
                let resolved = args.get("resolved").and_then(Value::as_bool) != Some(false);
                self.set_resolved(&id, resolved, window, cx);
            }
            "approve" => {
                let approve = args.get("approve").and_then(Value::as_bool) != Some(false);
                self.set_approved(approve, window, cx);
            }
            // A new thread on a line: {path, new} or {path, old} (a removed line).
            "line_comment" => {
                let fi = self.file_index(str_arg("path").context("missing path")?)?;
                let r = self.shown().context("no merge request loaded")?;
                let (new, old) = (num_arg("new").map(|n| n as u32), num_arg("old").map(|n| n as u32));
                let line = r.files[fi]
                    .body
                    .hunks()
                    .iter()
                    .flat_map(|h| &h.lines)
                    .find(|l| match new {
                        Some(n) => l.new == Some(n),
                        None => l.old == old && l.new.is_none(),
                    })
                    .context("no such line in the diff")?;
                let key = (fi, line.old, line.new);
                self.open_line_comment(key, window, cx);
                if let Some(body) = str_arg("body") {
                    self.boxes.line.update(cx, |b, cx| b.set_value(body.to_string(), window, cx));
                    if args.get("send").and_then(Value::as_bool) != Some(false) {
                        self.send_line_comment(window, cx);
                    }
                }
            }
            "cancel" => {
                self.close_reply(cx);
                self.close_line_comment(cx);
            }
            "sign_in" => self.sign_in(
                str_arg("host").unwrap_or(&self.host()).to_string(),
                str_arg("token").context("missing token")?.to_string(),
                cx,
            ),
            other => bail!(
                "unknown command {other:?} (state, refresh, back, list, open, section, file, fold, viewed, tree, parallel, next_thread, prev_thread, expand, thread, comment, reply, resolve, approve, line_comment, cancel, sign_in)"
            ),
        }
        cx.notify();
        Ok(self.state(cx))
    }
}

// ---- rendering ---------------------------------------------------------

/// A GitLab default button.
fn button(id: &'static str, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .h(px(32.))
        .px_3()
        .rounded(px(8.))
        .bg(ca(0x89888d66))
        .cursor_pointer()
        .text_color(c(TEXT))
        .hover(|s| s.bg(ca(0x89888d99)))
        .child(label)
}

impl GitLabView {
    /// `alice / review-fixtures / Merge requests / !1`
    fn breadcrumb(&self, cx: &mut Context<Self>) -> Div {
        let sep = || div().text_color(c(MUTED)).child("/");
        let narrow = self.width.get() < px(800.);
        // In a narrow pane the crumbs are cut, not the buttons after them.
        let mut crumbs = div()
            .flex_1()
            .min_w(px(0.))
            .overflow_hidden()
            .flex()
            .items_center()
            .gap_2()
            .whitespace_nowrap()
            .text_color(c(TEXT))
            .text_size(px(13.));
        // The host first, unless the pane is narrow: it says least.
        let mut parts: Vec<String> = vec![];
        if self.width.get() >= px(800.) || !self.in_mr() {
            let host = self.host();
            parts.push(host.trim_start_matches("https://").trim_start_matches("http://").to_string());
        }
        if let (Some(r), true) = (self.review.ready(), self.in_mr()) {
            let full = &r.mr.references.full;
            let project = full.rsplit_once('!').map_or(full.as_str(), |(p, _)| p);
            parts.extend(project.split('/').map(str::to_string));
        }
        for (i, part) in parts.into_iter().enumerate() {
            if i > 0 {
                crumbs = crumbs.child(sep());
            }
            crumbs = crumbs.child(div().when(i == 0, |d| d.text_color(c(MUTED))).child(part));
        }
        crumbs = crumbs.child(sep()).child(
            div()
                .id("gl-crumb-mrs")
                .cursor_pointer()
                .hover(|s| s.underline())
                .child("Merge requests")
                .on_click(cx.listener(|v, _, _, cx| v.back(cx))),
        );
        if let Some(iid) = self.loc.iid {
            crumbs = crumbs.child(sep()).child(div().font_weight(FontWeight::SEMIBOLD).child(format!("!{iid}")));
        }
        div()
            .flex()
            .items_center()
            .gap_2()
            .px_4()
            .py_2()
            .flex_none()
            .child(crumbs)
            .when_some(self.me.as_ref(), |d, me| {
                d.child(self.avatars.of(me, 24.)).when(!narrow, |d| {
                    d.child(div().flex_none().text_color(c(MUTED)).child(format!("@{}", me.username)))
                })
            })
            .child(
                // Just the icon in a narrow pane.
                button("gl-open-web", if narrow { "" } else { "Open in GitLab" })
                    .flex_none()
                    .gap_2()
                    .child(icon("external-link", 14., c(TEXT)))
                    .flex_row_reverse()
                    .on_click(cx.listener(|v, _, window, cx| {
                        let url = v.web_url();
                        abromux_plugin::open_url(&url, window, cx);
                    })),
            )
            .child(button("gl-refresh", "Refresh").flex_none().on_click(cx.listener(|v, _, _, cx| v.refresh(cx))))
    }

    fn sign_in_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some((host, token)) = self.sign_in.clone() else {
            return div().into_any_element();
        };
        let hint = format!(
            "Make one at {}/-/user_settings/personal_access_tokens with the api scope \
             (read_api only reads). \
             It's kept in your Keychain.",
            host.read(cx).value()
        );
        div()
            .size_full()
            .flex()
            .justify_center()
            .pt(px(80.))
            .child(
                div()
                    .w(px(420.))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(25.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(c(HEADING))
                            .child("Sign in to GitLab"),
                    )
                    .child(div().font_weight(FontWeight::SEMIBOLD).text_color(c(TEXT)).child("GitLab URL"))
                    .child(Input::new(&host))
                    .child(div().font_weight(FontWeight::SEMIBOLD).text_color(c(TEXT)).child("Access token"))
                    .child(Input::new(&token))
                    .child(div().text_size(px(12.)).text_color(c(MUTED)).child(hint))
                    .when_some(self.sign_in_error.clone(), |d, e| d.child(div().text_color(c(RED)).child(e)))
                    .child(
                        div()
                            .id("gl-sign-in")
                            .flex()
                            .justify_center()
                            .h(px(32.))
                            .items_center()
                            .rounded(px(8.))
                            .bg(c(0x1068bf))
                            .cursor_pointer()
                            .text_color(c(HEADING))
                            .child("Connect")
                            .on_click(cx.listener(move |v, _, _, cx| {
                                let h = host.read(cx).value().to_string();
                                let t = token.read(cx).value().to_string();
                                v.sign_in(h, t, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    fn status_line(&self, load: &str, error: Option<&str>, cx: &mut Context<Self>) -> Option<AnyElement> {
        match (load, error) {
            ("loading", _) => Some(spinner("gl-loading").into_any_element()),
            (_, Some(e)) => Some(self.alert(e, cx).into_any_element()),
            _ => None,
        }
    }

    /// GitLab's danger alert, with what can be done about the error.
    fn alert(&self, error: &str, cx: &mut Context<Self>) -> Div {
        let rejected = error.contains("(401)");
        div().p_4().child(
            div()
                .flex()
                .gap_3()
                .p_4()
                .rounded(px(8.))
                .bg(c(0x3e1d22))
                .border_1()
                .border_color(c(0x7a2e3a))
                .child(app_icon("circle-x", 16., c(RED)).mt(px(2.)))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(c(TEXT))
                                .child(if rejected { "GitLab didn't accept the token" } else { "Couldn't load this" }),
                        )
                        .child(div().text_color(c(TEXT)).child(error.to_string()))
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .when(rejected, |d| {
                                    d.child(button("gl-sign-in-again", "Sign in again").on_click(
                                        cx.listener(|v, _, window, cx| v.show_sign_in(window, cx)),
                                    ))
                                })
                                .child(button("gl-try-again", "Try again").on_click(cx.listener(|v, _, _, cx| v.refresh(cx)))),
                        ),
                ),
        )
    }

    fn list_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let mono = self.mono(cx);
        let scopes = [
            (ListScope::ToReview, "Reviews"),
            (ListScope::Mine, "Created by me"),
            (ListScope::All, "All"),
        ];
        let count = self.list.ready().map(Vec::len);
        let states = [MrState::Opened, MrState::Merged, MrState::Closed, MrState::All];
        let state_switch = div().flex().gap(px(1.)).rounded(px(8.)).overflow_hidden().children(states.map(|st| {
            let on = self.loc.state == st;
            div()
                .id(st.label())
                .flex()
                .items_center()
                .h(px(32.))
                .px_3()
                .cursor_pointer()
                .text_color(c(TEXT))
                .bg(if on { ca(0x89888d99) } else { ca(0x89888d40) })
                .when(on, |d| d.font_weight(FontWeight::SEMIBOLD))
                .hover(|s| s.bg(ca(0x89888d99)))
                .child(st.label())
                .on_click(cx.listener(move |v, _, _, cx| v.set_state(st, cx)))
        }));
        let tabs = div().flex().items_center().border_b_1().border_color(c(RAISED)).children(scopes.map(|(s, label)| {
            let on = self.loc.scope == s;
            div()
                .id(label)
                .flex()
                .items_center()
                .gap_2()
                .h(px(48.))
                .px_3()
                .border_b_2()
                .border_color(if on { c(HEADING) } else { transparent_black() })
                .cursor_pointer()
                .text_color(c(TEXT))
                .child(label)
                .when_some(count.filter(|_| on), |d, n| d.child(count_badge(n)))
                .on_click(cx.listener(move |v, _, _, cx| v.set_scope(s, cx)))
        }));
        let tabs = tabs.child(div().flex_1()).child(state_switch);
        let status = self.status_line(self.list.name(), self.list.error(), cx);
        let now = time::now();
        let query = self.list_search.read(cx).value().trim().to_lowercase();
        let empty = match self.loc.state {
            MrState::Opened => "No open merge requests here.",
            MrState::Merged => "No merged merge requests here.",
            MrState::Closed => "No closed merge requests here.",
            MrState::All => "No merge requests here.",
        };
        let rows = self.list.ready().map(|mrs| {
            if mrs.is_empty() {
                return vec![div().py_6().flex().justify_center().text_color(c(MUTED)).child(empty).into_any_element()];
            }
            // The filter: words that must all be in the title, reference, author or branches.
            let shown: Vec<&MergeRequest> = mrs
                .iter()
                .filter(|m| {
                    let hay = format!(
                        "{} {} {} {} {} {}",
                        m.title, m.references.full, m.author.name, m.author.username, m.source_branch, m.target_branch
                    )
                    .to_lowercase();
                    query.split_whitespace().all(|w| hay.contains(w))
                })
                .collect();
            if shown.is_empty() {
                return vec![div().py_6().flex().justify_center().text_color(c(MUTED)).child("No results found.").into_any_element()];
            }
            shown
                .into_iter()
                .map(|m| {
                    let (p, iid) = (m.project_id, m.iid);
                    let tasks = m.task_completion_status.as_ref().filter(|t| t.count > 0);
                    // Assignees, then reviewers, as GitLab's row shows them.
                    let people: Vec<&User> = m.assignees.iter().chain(&m.reviewers).collect();
                    div()
                        .id(SharedString::from(format!("mr-{}", m.id)))
                        .flex()
                        .items_center()
                        .gap_3()
                        .px_4()
                        .py(px(10.))
                        .border_b_1()
                        .border_color(c(RAISED))
                        .cursor_pointer()
                        .hover(|s| s.bg(c(CARD)))
                        .on_click(cx.listener(move |v, _, _, cx| v.open_mr(p, iid, cx)))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .flex()
                                        .items_baseline()
                                        .gap_3()
                                        .child(div().font_weight(FontWeight::SEMIBOLD).text_color(c(TEXT)).truncate().child(m.title.clone()))
                                        .when_some(tasks, |d, t| {
                                            d.child(div().flex_none().text_size(px(12.)).text_color(c(MUTED)).child(format!(
                                                "{} of {} checklist item{} completed",
                                                t.completed_count,
                                                t.count,
                                                if t.count == 1 { "" } else { "s" }
                                            )))
                                        }),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .items_center()
                                        .gap_1()
                                        .text_size(px(12.))
                                        .line_height(px(18.))
                                        .text_color(c(MUTED))
                                        .child(format!(
                                            "{} · created {} by {}",
                                            m.references.full,
                                            time::ago(&m.created_at, now),
                                            m.author.name
                                        ))
                                        // Into another branch than the default: which.
                                        .when(
                                            self.default_branches
                                                .get(&m.project_id)
                                                .is_some_and(|d| !d.is_empty() && *d != m.target_branch),
                                            |d| {
                                                d.child(
                                                    div()
                                                        .flex()
                                                        .items_center()
                                                        .gap_1()
                                                        .ml_1()
                                                        .child(app_icon("git-branch", 12., c(MUTED)))
                                                        .child(div().font_family(mono.clone()).child(m.target_branch.clone())),
                                                )
                                            },
                                        )
                                        .children(m.labels.iter().map(label_pill)),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .items_end()
                                .text_size(px(12.))
                                .line_height(px(18.))
                                .text_color(c(MUTED))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .when_some(self.list_pipelines.get(&m.id), |d, p| d.child(pipeline_icon(p, 16.)))
                                        .child(div().flex().gap_1().children(people.iter().map(|u| self.avatars.of(u, 16.))))
                                        .map(|d| match self.list_threads.get(&m.id) {
                                            // GitLab's grey "1 of 4" pill of resolved threads.
                                            Some(&(done, all)) if all > 0 => d.child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap_1()
                                                    .px(px(6.))
                                                    .rounded(px(6.))
                                                    .bg(c(if done == all { 0x0d532a } else { 0x4c4b51 }))
                                                    .text_color(c(if done == all { 0x91d4a8 } else { TEXT }))
                                                    .child(icon("comment", 12., c(if done == all { 0x91d4a8 } else { TEXT })))
                                                    .child(format!("{done} of {all}")),
                                            ),
                                            _ if m.user_notes_count > 0 => d.child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap_1()
                                                    .text_size(px(14.))
                                                    .text_color(c(TEXT))
                                                    .child(icon("comment", 14., c(TEXT)))
                                                    .child(m.user_notes_count.to_string()),
                                            ),
                                            _ => d,
                                        })
                                        .when(m.state == "merged", |d| d.child(badge("Merged", c(0x64a7e9), c(0x1d2153))))
                                        .when(m.state == "closed", |d| d.child(badge("Closed", c(0xfc7988), c(0x5a0e17))))
                                        .when(self.approved.contains(&m.id) && m.state == "opened", |d| {
                                            d.child(badge("Approved", c(GREEN), c(0x002c26)).child(icon("check-circle", 12., c(0x002c26))).flex_row_reverse())
                                        }),
                                )
                                .child(div().flex().gap_1().child("updated").child(when(&m.updated_at))),
                        )
                        .into_any_element()
                })
                .collect()
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .px_4()
                    .pt_2()
                    .text_size(px(25.))
                    .line_height(px(31.25))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c(HEADING))
                    .child("Merge requests"),
            )
            .child(div().px_4().pt_2().child(tabs))
            .child(
                div().px_4().py_3().border_b_1().border_color(c(RAISED)).child(
                    Input::new(&self.list_search).prefix(app_icon("search", 14., c(MUTED))).cleanable(true),
                ),
            )
            .child(
                div()
                    .id("gl-list")
                    .flex_1()
                    .min_h(px(0.))
                    .px_4()
                    .overflow_y_scroll()
                    .children(status)
                    .children(rows.unwrap_or_default()),
            )
            .into_any_element()
    }

    fn mr_page(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if let Some(s) = self.status_line(self.review.name(), self.review.error(), cx) {
            return s;
        }
        let Some(r) = self.review.ready().cloned() else {
            return div().into_any_element();
        };
        let body = match self.loc.section {
            Section::Overview => self.overview_page(&r, window, cx),
            Section::Changes => self.changes_page(&r, cx),
            Section::Commits => self.commits_page(&r, cx),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.mr_header(&r, cx))
            .child(div().flex_1().min_h(px(0.)).child(body))
            .into_any_element()
    }
}

impl Render for GitLabView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = if self.sign_in.is_some() {
            self.sign_in_page(cx)
        } else if self.in_mr() {
            self.mr_page(window, cx)
        } else {
            self.list_page(cx)
        };
        // Measure the tab; a change of width redraws it with the right layout.
        let width = self.width.clone();
        let me = cx.entity_id();
        let measure = canvas(
            move |bounds, _, cx| {
                if (width.get() - bounds.size.width).abs() > px(1.) {
                    width.set(bounds.size.width);
                    cx.notify(me);
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        div()
            .id("gitlab")
            .relative()
            .child(measure)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|v, ev: &KeyDownEvent, window, cx| v.key(ev, window, cx)))
            // A click anywhere in the tab gives it the keys.
            .on_mouse_down(MouseButton::Left, cx.listener(|v, _, window, _| window.focus(&v.focus)))
            .size_full()
            .flex()
            .flex_col()
            .bg(c(BG))
            .text_size(px(14.))
            .line_height(px(21.))
            .text_color(c(TEXT))
            .when(self.sign_in.is_none(), |d| d.child(self.breadcrumb(cx)))
            .child(div().flex_1().min_h(px(0.)).child(page))
    }
}
