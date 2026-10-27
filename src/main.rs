mod activity;
mod agent;
mod banner;
mod bench;
mod browser;
mod browser_auto;
mod browser_find;
mod channel;
mod cli;
mod config;
mod debug;
mod ghostty;
mod icon;
mod ipc;
mod link;
mod notification;
mod osc99;
mod palette;
mod quit;
mod server;
mod session;
mod shell;
mod skills;
mod sprite;
mod status;
mod terminal;
mod theme;
mod titlebar;
mod webauthn;
mod workspace;

use browser::{BrowserEvent, BrowserView};
use futures::StreamExt;
use gpui::{prelude::*, *};
use gpui_component::{
    Root, Theme, ThemeMode,
    input::{Input, InputEvent, InputState, SelectAll},
};
use std::{
    borrow::Cow,
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    time::{Duration, Instant},
};
use terminal::{TerminalEvent, TerminalView};
use theme::*;
use workspace::{Dir, Node, Pane, Surface, TabDrop, Workspace, git_branch, tilde};

actions!(
    abromux,
    [
        Quit,
        NewWorkspace,
        CloseWorkspace,
        RenameWorkspace,
        NextWorkspace,
        PrevWorkspace,
        NewTerminal,
        NewBrowser,
        SplitRight,
        SplitDown,
        CloseTab,
        NextTab,
        PrevTab,
        ToggleSidebar,
        FocusAddressBar,
        Ws1,
        Ws2,
        Ws3,
        Ws4,
        Ws5,
        Ws6,
        Ws7,
        Ws8,
        Ws9,
        Tab1,
        Tab2,
        Tab3,
        Tab4,
        Tab5,
        Tab6,
        Tab7,
        Tab8,
        Tab9,
        FocusLeft,
        FocusRight,
        FocusUp,
        FocusDown,
        ResizeLeft,
        ResizeRight,
        ResizeUp,
        ResizeDown,
        FlashPane,
        ToggleZoom,
        ToggleNotifications,
        JumpToUnread,
        CommandPalette,
        WorkspaceSwitcher,
        RenameTab
    ]
);

/// New browsers open blank, with the address bar focused.
const DEFAULT_URL: &str = "";
/// Pixels a divider moves per ⌃⇧H/J/K/L press (cmux's `paneResizeStepPixels` default).
pub const RESIZE_STEP: f32 = 20.;
/// How long a lone ⌘ or ⌃ must be held before shortcut hints show (cmux's value).
const HINT_DELAY: Duration = Duration::from_millis(300);
/// Sidebar width: default and the range its edge can be dragged within.
const SIDEBAR_WIDTH: f32 = 232.;
const SIDEBAR_MIN: f32 = 180.;
const SIDEBAR_MAX: f32 = 480.;
/// Height of the titlebar row the traffic lights sit in.
const TITLEBAR_H: f32 = 38.;
const TABBAR_H: f32 = 30.;
/// How close to the pane area's outer edge a dragged tab splits the whole
/// workspace instead of one pane.
const DROP_EDGE: f32 = 32.;

/// Which shortcut hints are showing: ⌘ numbers workspaces, ⌃ numbers the
/// focused pane's tabs.
#[derive(Clone, Copy, PartialEq)]
enum Hints {
    Command,
    Control,
}

struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        macro_rules! embed {
            ($($name:literal),*) => {
                match path {
                    $(concat!("icons/", $name, ".svg") => Some(include_bytes!(concat!("../assets/icons/", $name, ".svg")).as_slice()),)*
                    _ => None,
                }
            };
        }
        let bytes = embed!(
            "terminal",
            "globe",
            "plus",
            "close",
            "x",
            "split-right",
            "split-down",
            "chevron-left",
            "chevron-right",
            "chevron-down",
            "chevron-up",
            "rotate-cw",
            "panel-left",
            "bell",
            "git-branch",
            "folder",
            "search",
            "circle-x",
            "pencil",
            "maximize-2"
        );
        Ok(bytes.map(Cow::Borrowed))
    }

    fn list(&self, _: &str) -> Result<Vec<SharedString>> {
        Ok(vec![])
    }
}

struct AbroMux {
    workspaces: Vec<Workspace>,
    active: usize,
    next_id: usize,
    next_ws: usize,
    next_pane: usize,
    next_surface: usize,
    /// Stable `surface:N` numbers handed out to the control socket.
    surface_refs: HashMap<EntityId, usize>,
    socket: Option<std::path::PathBuf>,
    /// Last seen window frame, for the session file.
    window_bounds: Option<Bounds<Pixels>>,
    renaming: Option<(usize, Entity<InputState>)>,
    /// The tab being renamed in its tab bar.
    renaming_tab: Option<(EntityId, Entity<InputState>)>,
    sidebar: bool,
    sidebar_width: f32,
    resizing_sidebar: bool,
    split_bounds: Rc<RefCell<HashMap<usize, Bounds<Pixels>>>>,
    dragging: Option<(usize, Axis)>,
    /// Where a dragged tab would land: the pane under the cursor, the
    /// target, and the area to highlight.
    tab_drop: Option<(usize, TabDrop, Bounds<Pixels>)>,
    /// The pane area, for drops on its outer edges.
    main_bounds: Rc<Cell<Bounds<Pixels>>>,
    branches: RefCell<HashMap<String, (Instant, Option<String>)>>,
    hints: Option<Hints>,
    /// Bumped on every modifier change so a stale delayed show does nothing.
    hint_gen: usize,
    /// Pane being flashed (⌘⇧H), with a counter that restarts the animation.
    flash: Option<(usize, usize)>,
    focus: FocusHandle,
    /// Frame timings for `debug.frames`.
    frames: Rc<RefCell<debug::FrameLog>>,
    /// Oldest first (notification.rs).
    notifications: Vec<notification::Notification>,
    next_notification: usize,
    /// The ⌘I list is showing.
    notifications_open: bool,
    /// Unread count last put on the dock icon.
    badge: usize,
    /// The ⌘⇧P / ⌘P palette, while open.
    palette: Option<palette::Palette>,
}

impl AbroMux {
    fn new(
        cwd: Option<String>,
        session: Option<session::Session>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            workspaces: vec![],
            active: 0,
            next_id: 1,
            next_ws: 0,
            next_pane: 0,
            next_surface: 0,
            surface_refs: Default::default(),
            socket: None,
            window_bounds: None,
            renaming: None,
            renaming_tab: None,
            sidebar: true,
            sidebar_width: SIDEBAR_WIDTH,
            resizing_sidebar: false,
            split_bounds: Default::default(),
            dragging: None,
            tab_drop: None,
            main_bounds: Default::default(),
            branches: Default::default(),
            hints: None,
            hint_gen: 0,
            flash: None,
            focus: cx.focus_handle(),
            frames: Default::default(),
            notifications: vec![],
            next_notification: 0,
            notifications_open: false,
            badge: 0,
            palette: None,
        };
        cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active() {
                this.set_hints(None, cx);
            }
            // Coming back marks what you're looking at as read.
            cx.notify();
        })
        .detach();
        let mut banner_clicks = banner::init();
        cx.spawn_in(window, async move |this, cx| {
            while let Some(id) = banner_clicks.next().await {
                if this
                    .update_in(cx, |this, window, cx| {
                        this.open_notification(id, window, cx)
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        let mut clicks = browser::watch_clicks();
        cx.spawn_in(window, async move |this, cx| {
            while clicks.next().await.is_some() {
                if this
                    .update_in(cx, |this, window, cx| this.follow_page_focus(window, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        this.socket = server::start(window, cx);
        let restored = session.is_some_and(|s| this.restore(s, window, cx));
        if !restored || cwd.is_some() {
            this.new_workspace(cwd, window, cx);
        }
        this.persist(window, cx);
        if std::env::var("ABROMUX_DEMO").is_ok() {
            this.demo(window, cx);
        }
        if let Ok(script) = std::env::var("ABROMUX_SCRIPT") {
            debug::run_script(script, Self::script_mouse, window, cx);
        }
        this
    }

    /// A click into a page gives its WKWebView native focus behind gpui's back.
    /// Move gpui focus there too, or the address bar keeps taking ⌘A/⌘C/⌘V.
    fn follow_page_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let hit = self
            .ws()
            .root
            .panes()
            .into_iter()
            .filter_map(|p| p.active_surface())
            .find_map(|s| match s {
                Surface::Browser(b) if b.read(cx).page_has_focus(cx) => Some(b.clone()),
                _ => None,
            });
        if let Some(b) = hit {
            let focus = b.focus_handle(cx);
            if !focus.is_focused(window) {
                b.update(cx, |b, cx| b.page_focused(window, cx));
                self.focus_surface(b.entity_id(), cx);
            }
        }
    }

    /// Mouse and modifier steps of a test script go to the focused terminal.
    fn script_mouse(&mut self, ev: PlatformInput, window: &mut Window, cx: &mut Context<Self>) {
        if let PlatformInput::ModifiersChanged(m) = &ev {
            self.modifiers_changed(&m.modifiers, window, cx);
        } else if let Some(Surface::Terminal(t)) = self.ws().focused_surface().cloned() {
            t.update(cx, |t, cx| t.simulate_mouse(ev, window, cx));
        }
    }

    fn demo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let send = |s: Option<&Surface>, cmd: &str, cx: &mut Context<Self>| {
            if let Some(Surface::Terminal(t)) = s {
                t.update(cx, |t, _| t.send(cmd.as_bytes()));
            }
        };
        let repo = env!("CARGO_MANIFEST_DIR").to_string();
        send(self.ws().focused_surface(), "ls -la\r", cx);
        self.split(Axis::Horizontal, true, window, cx);
        let first = self.ws().root.panes()[0].id;
        self.workspaces[self.active].focused_pane = first;
        self.split(Axis::Vertical, false, window, cx);
        send(
            self.ws().focused_surface(),
            &format!(
                "cd {repo} && printf '\\e[1;32mlibghostty-vt\\e[0m + \\e[1;34mgpui\\e[0m\\n' && ls src\r"
            ),
            cx,
        );
        self.workspaces[self.active].focused_pane = first;

        self.new_workspace(Some(repo.clone()), window, cx);
        self.workspaces[self.active].custom_name = Some("abromux".into());
        send(
            self.ws().focused_surface(),
            "cargo --version && wc -l src/*.rs\r",
            cx,
        );
        let parent = std::path::Path::new(&repo)
            .parent()
            .unwrap()
            .to_string_lossy()
            .to_string();
        self.new_workspace(Some(format!("{parent}/gpui-experiments")), window, cx);
        send(self.ws().focused_surface(), "printf '\\a'\r", cx);
        self.select_workspace(0, window, cx);
    }

    /// Keep the session file current: layout every few seconds, everything
    /// (scrollback too) when the window closes or the app quits.
    fn persist(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if session::path().is_none() {
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            let mut last = None;
            loop {
                Timer::after(Duration::from_secs(5)).await;
                let Ok(s) = this.update(cx, |this, cx| this.snapshot(false, cx)) else {
                    break;
                };
                if last.as_ref() != Some(&s) {
                    session::save(&s);
                    last = Some(s);
                }
            }
        })
        .detach();
        cx.on_app_quit(|this, cx| {
            session::save(&this.snapshot(true, cx));
            async {}
        })
        .detach();
        let this = cx.entity().downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            if let Some(this) = this.upgrade() {
                session::save(&this.read(cx).snapshot(true, cx));
            }
            true
        });
    }

    /// Split ids (pane, workspace and surface ids have their own counters).
    fn id(&mut self) -> usize {
        self.next_id += 1;
        self.next_id
    }

    fn pane_id(&mut self) -> usize {
        self.next_pane += 1;
        self.next_pane
    }

    // ---- surfaces -------------------------------------------------------

    fn surface_ref(&mut self, id: EntityId) -> usize {
        let next = self.next_surface + 1;
        let r = *self.surface_refs.entry(id).or_insert(next);
        self.next_surface = self.next_surface.max(r);
        r
    }

    /// A shell in `cwd`; `ws` is the owning workspace's id (exported to the
    /// shell so `abromux` run inside it targets its own workspace and surface).
    fn make_terminal(
        &mut self,
        cwd: Option<String>,
        ws: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Surface {
        let surface = self.next_surface + 1;
        let mut env = vec![
            ("ABROMUX_WORKSPACE_ID".into(), format!("workspace:{ws}")),
            ("ABROMUX_SURFACE_ID".into(), format!("surface:{surface}")),
        ];
        if let Some(path) = &self.socket {
            env.push((
                "ABROMUX_SOCKET_PATH".into(),
                path.to_string_lossy().into_owned(),
            ));
        }
        let term = cx.new(|cx| TerminalView::new(cwd, env, cx));
        self.surface_ref(term.entity_id());
        cx.subscribe_in(&term, window, |this, term, ev, window, cx| {
            let id = term.entity_id();
            match ev {
                TerminalEvent::Exited => this.close_surface(id, window, cx),
                TerminalEvent::Focused => this.focus_surface(id, cx),
                TerminalEvent::OpenLink(target) => this.open_link(id, target.clone(), window, cx),
                TerminalEvent::Bell => {
                    this.add_notification(id, String::new(), String::new(), true, window, cx);
                }
                TerminalEvent::Notification { title, body } => {
                    this.add_notification(id, title.clone(), body.clone(), false, window, cx);
                }
                TerminalEvent::TitleChanged => cx.notify(),
                TerminalEvent::Respawned => {
                    // Status pills described what ran in the old shell.
                    this.forget_status(id);
                    cx.notify();
                }
            }
        })
        .detach();
        Surface::Terminal(term)
    }

    fn make_browser(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) -> Surface {
        let browser = cx.new(|cx| BrowserView::new(url, window, cx));
        self.surface_ref(browser.entity_id());
        cx.subscribe_in(&browser, window, |this, b, ev, _, cx| match ev {
            BrowserEvent::TitleChanged => cx.notify(),
            BrowserEvent::Focused => this.focus_surface(b.entity_id(), cx),
        })
        .detach();
        Surface::Browser(browser)
    }

    /// ⌘-click on a terminal link: web URLs open in a browser to the right
    /// of the terminal (a new tab there if that pane already shows one),
    /// anything else with its default app.
    fn open_link(
        &mut self,
        from: EntityId,
        target: link::Target,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let url = match target {
            link::Target::Url(u) if u.starts_with("http://") || u.starts_with("https://") => u,
            link::Target::Url(u) => {
                let _ = std::process::Command::new("/usr/bin/open").arg(u).spawn();
                return;
            }
            link::Target::Path(p, _) => {
                let _ = std::process::Command::new("/usr/bin/open").arg(p).spawn();
                return;
            }
        };
        let Some(wi) = self.workspaces.iter().position(|w| w.contains(from)) else {
            return;
        };
        let ws = &self.workspaces[wi];
        let Some(pane) = ws
            .root
            .panes()
            .into_iter()
            .find(|p| p.tabs.iter().any(|s| s.entity_id() == from))
            .map(|p| p.id)
        else {
            return;
        };
        let right = ws.root.neighbor(pane, Dir::Right).filter(|&r| {
            ws.root
                .panes()
                .into_iter()
                .find(|p| p.id == r)
                .and_then(|p| p.active_surface())
                .is_some_and(|s| matches!(s, Surface::Browser(_)))
        });
        let surface = self.make_browser(&url, window, cx);
        match right {
            Some(r) => self.add_tab_to(wi, r, surface, true, window, cx),
            None => {
                self.split_pane(wi, pane, Axis::Horizontal, false, surface, true, window, cx);
            }
        }
    }

    /// Make the pane containing `id` the focused pane (and select its tab).
    fn focus_surface(&mut self, id: EntityId, cx: &mut Context<Self>) {
        for (wi, ws) in self.workspaces.iter_mut().enumerate() {
            let hit = ws.root.panes().iter().find_map(|p| {
                p.tabs
                    .iter()
                    .position(|s| s.entity_id() == id)
                    .map(|t| (p.id, t))
            });
            if let Some((pane, tab)) = hit {
                ws.focused_pane = pane;
                if let Some(p) = ws.root.pane_mut(pane) {
                    p.active = tab;
                }
                if wi == self.active {
                    cx.notify();
                }
                return;
            }
        }
    }

    fn close_surface(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(wi) = self.workspaces.iter().position(|w| w.contains(id)) else {
            return;
        };
        self.forget_notifications(id);
        self.forget_status(id);
        let ws = &mut self.workspaces[wi];
        self.surface_refs.remove(&id);
        let pane_id = ws
            .root
            .panes()
            .iter()
            .find(|p| p.tabs.iter().any(|s| s.entity_id() == id))
            .map(|p| p.id)
            .unwrap();
        let pane = ws.root.pane_mut(pane_id).unwrap();
        let idx = pane.tabs.iter().position(|s| s.entity_id() == id).unwrap();
        pane.tabs.remove(idx);
        if pane.active >= pane.tabs.len() {
            pane.active = pane.tabs.len().saturating_sub(1);
        } else if idx < pane.active {
            pane.active -= 1;
        }
        if pane.tabs.is_empty() {
            if !ws.root.remove(pane_id) {
                // Last pane in the workspace: closing it closes the workspace.
                self.close_workspace(wi, window, cx);
                return;
            }
            if ws.focused_pane == pane_id {
                ws.focused_pane = ws.root.panes()[0].id;
            }
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    // ---- workspaces -----------------------------------------------------

    fn ws(&self) -> &Workspace {
        &self.workspaces[self.active]
    }

    fn new_workspace(&mut self, cwd: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.create_workspace(cwd, true, window, cx);
    }

    /// Returns the new workspace's index.
    fn create_workspace(
        &mut self,
        cwd: Option<String>,
        select: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> usize {
        self.next_ws += 1;
        let id = self.next_ws;
        let surface = self.make_terminal(cwd, id, window, cx);
        let pane = Pane {
            id: self.pane_id(),
            tabs: vec![surface],
            active: 0,
        };
        let focused = pane.id;
        let ws = Workspace::new(id, None, Node::Leaf(pane), focused);
        self.workspaces.push(ws);
        let index = self.workspaces.len() - 1;
        if select {
            self.select_workspace(index, window, cx);
        }
        cx.notify();
        index
    }

    fn close_workspace(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.workspaces.len() {
            return;
        }
        let ws = self.workspaces.remove(index);
        // Hide any webviews before they drop so nothing lingers on screen.
        for p in ws.root.panes() {
            for s in &p.tabs {
                self.surface_refs.remove(&s.entity_id());
                self.notifications.retain(|n| n.surface != s.entity_id());
                if let Surface::Browser(b) = s {
                    b.update(cx, |b, cx| b.set_shown(false, cx));
                }
            }
        }
        if self.workspaces.is_empty() {
            self.new_workspace(None, window, cx);
            return;
        }
        if self.active > index || self.active >= self.workspaces.len() {
            self.active = self.active.saturating_sub(1);
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    fn select_workspace(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index < self.workspaces.len() {
            self.active = index;
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    fn start_rename(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ws) = self.workspaces.get(index) else {
            return;
        };
        let (id, name) = (ws.id, ws.name(cx));
        let input = cx.new(|cx| InputState::new(window, cx).default_value(name));
        cx.subscribe_in(
            &input,
            window,
            move |this, input, ev: &InputEvent, window, cx| match ev {
                InputEvent::PressEnter { .. } => {
                    let name = one_line(&input.read(cx).value());
                    if let Some(ws) = this.workspaces.iter_mut().find(|w| w.id == id) {
                        // Leaving the name as it was keeps it following the agent's title.
                        let changed = !name.is_empty() && name != ws.auto_name(cx);
                        ws.custom_name = changed.then_some(name);
                    }
                    this.renaming = None;
                    this.focus_active(window, cx);
                    cx.notify();
                }
                InputEvent::Blur => {
                    this.renaming = None;
                    cx.notify();
                }
                _ => {}
            },
        )
        .detach();
        input.update(cx, |i, cx| i.focus(window, cx));
        // Select the old name so typing replaces it; the input has to be
        // rendered before it can take the action.
        window.on_next_frame(|window, cx| window.dispatch_action(Box::new(SelectAll), cx));
        self.renaming = Some((id, input));
        cx.notify();
    }

    /// Rename tab `id` inline in its tab bar. An empty name goes back to the
    /// program's title.
    fn start_tab_rename(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(surface) = self.find_tab(id) else {
            return;
        };
        let name = surface.title(cx);
        let input = cx.new(|cx| InputState::new(window, cx).default_value(name));
        cx.subscribe_in(
            &input,
            window,
            move |this, input, ev: &InputEvent, window, cx| match ev {
                InputEvent::PressEnter { .. } => {
                    let name = one_line(&input.read(cx).value());
                    if let Some(s) = this.find_tab(id) {
                        // Leaving the title as it was keeps it following the program.
                        let unchanged = s.custom_title(cx).is_none() && name == s.title(cx);
                        s.set_custom_title((!unchanged).then_some(name), cx);
                    }
                    this.renaming_tab = None;
                    this.focus_active(window, cx);
                    cx.notify();
                }
                InputEvent::Blur => {
                    this.renaming_tab = None;
                    cx.notify();
                }
                _ => {}
            },
        )
        .detach();
        input.update(cx, |i, cx| i.focus(window, cx));
        window.on_next_frame(|window, cx| window.dispatch_action(Box::new(SelectAll), cx));
        self.renaming_tab = Some((id, input));
        cx.notify();
    }

    fn find_tab(&self, id: EntityId) -> Option<Surface> {
        self.workspaces
            .iter()
            .flat_map(|w| w.root.panes())
            .flat_map(|p| p.tabs.iter())
            .find(|s| s.entity_id() == id)
            .cloned()
    }

    fn focus_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = self
            .workspaces
            .get(self.active)
            .and_then(|w| w.focused_surface())
            .cloned()
        {
            // Defer so newly-created views have been laid out first.
            window.defer(cx, move |window, cx| s.focus(window, cx));
        }
    }

    // ---- panes ----------------------------------------------------------

    fn focused_cwd(&self, cx: &App) -> Option<String> {
        self.ws().cwd(cx)
    }

    fn add_tab(&mut self, surface: Surface, window: &mut Window, cx: &mut Context<Self>) {
        let pane = self.ws().focused_pane;
        self.add_tab_to(self.active, pane, surface, true, window, cx);
    }

    /// Insert `surface` after the active tab of pane `pane_id` in workspace `wi`.
    fn add_tab_to(
        &mut self,
        wi: usize,
        pane_id: usize,
        surface: Surface,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ws = &mut self.workspaces[wi];
        let Some(pane) = ws.root.pane_mut(pane_id) else {
            return;
        };
        let at = (pane.active + 1).min(pane.tabs.len());
        pane.tabs.insert(at, surface);
        if focus {
            pane.active = at;
            ws.focused_pane = pane_id;
            if wi == self.active {
                self.focus_active(window, cx);
            }
        }
        cx.notify();
    }

    fn split(&mut self, axis: Axis, browser: bool, window: &mut Window, cx: &mut Context<Self>) {
        let surface = if browser {
            self.make_browser(DEFAULT_URL, window, cx)
        } else {
            let cwd = self.focused_cwd(cx);
            let ws = self.ws().id;
            self.make_terminal(cwd, ws, window, cx)
        };
        let pane = self.ws().focused_pane;
        self.split_pane(self.active, pane, axis, false, surface, true, window, cx);
    }

    /// Split pane `pane_id` of workspace `wi`, putting `surface` in the new
    /// pane (left/up of it with `before`). Returns the new pane's id.
    #[allow(clippy::too_many_arguments)]
    fn split_pane(
        &mut self,
        wi: usize,
        pane_id: usize,
        axis: Axis,
        before: bool,
        surface: Surface,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        let pane = Pane {
            id: self.pane_id(),
            tabs: vec![surface],
            active: 0,
        };
        let (new_id, split_id) = (pane.id, self.id());
        let ws = &mut self.workspaces[wi];
        ws.root.split(pane_id, axis, before, pane, split_id).ok()?;
        if focus {
            ws.focused_pane = new_id;
            if wi == self.active {
                self.focus_active(window, cx);
            }
        }
        cx.notify();
        Some(new_id)
    }

    fn move_tab(&mut self, id: EntityId, to: TabDrop, window: &mut Window, cx: &mut Context<Self>) {
        let (pane, split) = (self.pane_id(), self.id());
        if self.workspaces[self.active].move_tab(id, to, pane, split) {
            self.focus_active(window, cx);
        }
        self.tab_drop = None;
        cx.notify();
    }

    /// A tab dragged over pane `pane_id`'s content (`ev.bounds`): near the
    /// pane area's outer edge it splits the whole workspace, near the pane's
    /// own edge it splits the pane, and in the middle it joins its tabs.
    fn drag_over_pane(
        &mut self,
        pane_id: usize,
        ev: &DragMoveEvent<DraggedTab>,
        cx: &mut Context<Self>,
    ) {
        let (b, pos) = (ev.bounds, ev.event.position);
        if !b.contains(&pos) {
            if self.tab_drop.is_some_and(|(over, ..)| over == pane_id) {
                self.tab_drop = None;
                cx.notify();
            }
            return;
        }
        let ws = self.ws();
        let id = ev.drag(cx).sid;
        let half = |r: Bounds<Pixels>, dir: Dir| {
            let (w, h) = (r.size.width / 2., r.size.height / 2.);
            match dir {
                Dir::Left => Bounds::new(r.origin, size(w, r.size.height)),
                Dir::Right => {
                    Bounds::new(point(r.origin.x + w, r.origin.y), size(w, r.size.height))
                }
                Dir::Up => Bounds::new(r.origin, size(r.size.width, h)),
                Dir::Down => Bounds::new(point(r.origin.x, r.origin.y + h), size(r.size.width, h)),
            }
        };
        let m = self.main_bounds.get();
        // The top band starts below the top row's tab bars.
        let edge = [
            (Dir::Left, f32::from(pos.x - m.left())),
            (Dir::Right, f32::from(m.right() - pos.x)),
            (Dir::Up, f32::from(pos.y - m.top()) - TABBAR_H),
            (Dir::Down, f32::from(m.bottom() - pos.y)),
        ]
        .into_iter()
        .filter(|&(_, d)| d < DROP_EDGE)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(dir, _)| dir)
        .filter(|&dir| ws.root.panes().len() > 1 && ws.accepts(id, TabDrop::Edge(dir)));
        let target = if let Some(dir) = edge {
            Some((TabDrop::Edge(dir), half(m, dir)))
        } else {
            let fx = (pos.x - b.left()) / b.size.width;
            let fy = (pos.y - b.top()) / b.size.height;
            let (dir, d) = [
                (Dir::Left, fx),
                (Dir::Right, 1. - fx),
                (Dir::Up, fy),
                (Dir::Down, 1. - fy),
            ]
            .into_iter()
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap();
            let to = if d < 0.3 {
                TabDrop::Split { pane: pane_id, dir }
            } else {
                TabDrop::Tabs {
                    pane: pane_id,
                    index: usize::MAX,
                }
            };
            let rect = if d < 0.3 { half(b, dir) } else { b };
            ws.accepts(id, to).then_some((to, rect))
        };
        let next = target.map(|(to, rect)| (pane_id, to, rect));
        if next != self.tab_drop {
            self.tab_drop = next;
            cx.notify();
        }
    }

    fn cycle_tab(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let ws = &mut self.workspaces[self.active];
        if let Some(p) = ws.root.pane_mut(ws.focused_pane)
            && !p.tabs.is_empty()
        {
            p.active = (p.active as isize + delta).rem_euclid(p.tabs.len() as isize) as usize;
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    /// ⌃1–8 select that tab of the focused pane, ⌃9 the last one.
    fn select_tab(&mut self, index: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let ws = &mut self.workspaces[self.active];
        let Some(p) = ws.root.pane_mut(ws.focused_pane) else {
            return;
        };
        let i = index.unwrap_or(p.tabs.len().saturating_sub(1));
        if i < p.tabs.len() {
            p.active = i;
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    fn focus_dir(&mut self, dir: Dir, window: &mut Window, cx: &mut Context<Self>) {
        let ws = &mut self.workspaces[self.active];
        if let Some(id) = ws.root.neighbor(ws.focused_pane, dir) {
            ws.focused_pane = id;
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    /// Move the divider of pane `pane`'s innermost split along `dir`'s axis
    /// by `px` toward `dir`, as tmux's `resize-pane` does. False if the pane
    /// has no such split.
    fn resize_pane(
        &mut self,
        wi: usize,
        pane: usize,
        dir: Dir,
        px: f32,
        cx: &mut Context<Self>,
    ) -> bool {
        let (axis, sign) = match dir {
            Dir::Left => (Axis::Horizontal, -1.),
            Dir::Right => (Axis::Horizontal, 1.),
            Dir::Up => (Axis::Vertical, -1.),
            Dir::Down => (Axis::Vertical, 1.),
        };
        let root = &mut self.workspaces[wi].root;
        let Some(id) = root.split_along(pane, axis) else {
            return false;
        };
        let Some(b) = self.split_bounds.borrow().get(&id).copied() else {
            return false;
        };
        let size = f32::from(if axis == Axis::Horizontal {
            b.size.width
        } else {
            b.size.height
        });
        let Some(ratio) = root.split_mut(id) else {
            return false;
        };
        *ratio = (*ratio + sign * px / size.max(1.)).clamp(0.1, 0.9);
        cx.notify();
        true
    }

    fn resize_focused(&mut self, dir: Dir, cx: &mut Context<Self>) {
        let (wi, pane) = (self.active, self.ws().focused_pane);
        self.resize_pane(wi, pane, dir, RESIZE_STEP, cx);
    }

    fn flash_pane(&mut self, cx: &mut Context<Self>) {
        let n = self.flash.map_or(0, |(_, n)| n + 1);
        self.flash = Some((self.ws().focused_pane, n));
        cx.notify();
        cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_millis(700)).await;
            let _ = this.update(cx, |this, cx| {
                if this.flash.is_some_and(|(_, m)| m == n) {
                    this.flash = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Show hints after a lone ⌘ or ⌃ has been held for `HINT_DELAY`; any
    /// other modifier change hides them.
    fn modifiers_changed(&mut self, m: &Modifiers, window: &mut Window, cx: &mut Context<Self>) {
        let want = match (m.platform, m.control, m.alt || m.shift || m.function) {
            (true, false, false) => Some(Hints::Command),
            (false, true, false) => Some(Hints::Control),
            _ => None,
        };
        if want.is_some() && want == self.hints {
            return;
        }
        self.set_hints(None, cx);
        let Some(want) = want else { return };
        let generation = self.hint_gen;
        cx.spawn_in(window, async move |this, cx| {
            Timer::after(HINT_DELAY).await;
            let _ = this.update(cx, |this, cx| {
                if this.hint_gen == generation {
                    this.hints = Some(want);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn set_hints(&mut self, hints: Option<Hints>, cx: &mut Context<Self>) {
        self.hint_gen += 1;
        if self.hints != hints {
            self.hints = hints;
            cx.notify();
        }
    }

    /// Native webviews draw above gpui, so only the visible ones may be shown.
    fn sync_webviews(&self, cx: &mut App) {
        for (wi, ws) in self.workspaces.iter().enumerate() {
            for p in ws.root.panes() {
                for (ti, s) in p.tabs.iter().enumerate() {
                    if let Surface::Browser(b) = s {
                        let shown = wi == self.active && ti == p.active && ws.shows(p.id) && self.renaming.is_none() && !self.notifications_open
                            && self.palette.is_none()
                            // Hidden while dragging a tab so drop targets show
                            // over them and get the mouse.
                            && !cx.has_active_drag();
                        b.update(cx, |b, cx| b.set_shown(shown, cx));
                    }
                }
            }
        }
    }

    fn branch(&self, cwd: &str) -> Option<String> {
        let mut cache = self.branches.borrow_mut();
        if let Some((at, b)) = cache.get(cwd)
            && at.elapsed() < Duration::from_secs(3)
        {
            return b.clone();
        }
        let b = git_branch(cwd);
        cache.insert(cwd.to_string(), (Instant::now(), b.clone()));
        b
    }

    // ---- rendering ------------------------------------------------------

    fn render_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let rows: Vec<AnyElement> = self
            .workspaces
            .iter()
            .enumerate()
            .map(|(i, ws)| {
                let active = i == self.active;
                let name = ws.name(cx);
                let subtitle = ws
                    .focused_surface()
                    .map(|s| s.title(cx))
                    .unwrap_or_default();
                let cwd = ws.cwd(cx);
                let branch = cwd.as_deref().and_then(|c| self.branch(c));
                let meta = match (&branch, &cwd) {
                    (Some(b), Some(c)) => format!("{b} • {}", tilde(c)),
                    (None, Some(c)) => tilde(c),
                    _ => String::new(),
                };
                let fg = if active { rgb(0xffffff) } else { rgb(TEXT) };
                let sub = if active { rgba(0xffffffc0) } else { rgb(MUTED) };
                let renaming = self
                    .renaming
                    .as_ref()
                    .filter(|(id, _)| *id == ws.id)
                    .map(|(_, input)| input.clone());
                let is_renaming = renaming.is_some();
                let group: SharedString = format!("ws-{}", ws.id).into();
                let hint = shortcut_digit(i, self.workspaces.len())
                    .filter(|_| self.hints == Some(Hints::Command));
                let latest = self.latest_unread(ws).map(|n| n.text());
                let status = self.render_ws_status(ws, active);

                div()
                    .id(("ws", ws.id))
                    .group(group.clone())
                    .relative()
                    .mx(px(8.))
                    .px(px(10.))
                    .py(px(7.))
                    .rounded(px(7.))
                    .cursor_pointer()
                    .when(active, |s| s.bg(rgb(ACCENT)))
                    .when(!active, |s| s.hover(|s| s.bg(rgba(HOVER))))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                            if ev.click_count >= 2 {
                                this.start_rename(i, window, cx);
                            } else {
                                this.select_workspace(i, window, cx);
                            }
                        }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .when(latest.is_some(), |s| {
                                s.child(
                                    div()
                                        .size(px(7.))
                                        .flex_none()
                                        .rounded_full()
                                        .bg(rgb(ACCENT_BRIGHT)),
                                )
                            })
                            .child(match renaming {
                                // Edit in place: same font and line as the title, in a
                                // dark inset so it reads as a field on the accent row.
                                Some(input) => div()
                                    .flex_1()
                                    .min_w_0()
                                    .h(px(20.))
                                    .my(px(-2.))
                                    .mx(px(-5.))
                                    .px(px(4.))
                                    .flex()
                                    .items_center()
                                    .rounded(px(5.))
                                    .bg(rgba(0x0000004d))
                                    .border_1()
                                    .border_color(rgba(0xffffff59))
                                    .text_color(rgb(TEXT))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation()
                                    })
                                    .capture_key_down(cx.listener(
                                        |this, ev: &KeyDownEvent, window, cx| {
                                            if ev.keystroke.key == "escape" {
                                                this.renaming = None;
                                                this.focus_active(window, cx);
                                                cx.notify();
                                            }
                                        },
                                    ))
                                    .child(
                                        Input::new(&input)
                                            .appearance(false)
                                            .text_size(px(12.5))
                                            .h_full()
                                            .px_0()
                                            .py_0(),
                                    )
                                    .into_any_element(),
                                None => div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(fg)
                                    .child(name.clone())
                                    .into_any_element(),
                            })
                            .when(!is_renaming, |s| {
                                s.child(
                                    div()
                                        .id(("ws-close", ws.id))
                                        .invisible()
                                        .group_hover(group.clone(), |s| s.visible())
                                        .flex_none()
                                        .rounded(px(4.))
                                        .p(px(2.))
                                        .hover(|s| s.bg(rgba(0xffffff22)))
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                            cx.stop_propagation()
                                        })
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.close_workspace(i, window, cx)
                                        }))
                                        .child(icon("x").size(px(11.)).text_color(sub)),
                                )
                            }),
                    )
                    .when(
                        !subtitle.is_empty()
                            && subtitle != name
                            && Some(&subtitle) != cwd.as_ref().map(|c| tilde(c)).as_ref(),
                        |s| {
                            s.child(
                                div()
                                    .mt(px(2.))
                                    .text_size(px(11.))
                                    .text_color(sub)
                                    .line_clamp(2)
                                    .child(subtitle),
                            )
                        },
                    )
                    .when_some(latest, |s, text| {
                        s.child(
                            div()
                                .mt(px(2.))
                                .flex()
                                .gap(px(4.))
                                .text_size(px(11.))
                                .text_color(if active {
                                    rgb(0xffffff)
                                } else {
                                    rgb(ACCENT_BRIGHT)
                                })
                                .child(icon("bell").mt(px(1.)).size(px(10.)).text_color(
                                    if active {
                                        rgb(0xffffff)
                                    } else {
                                        rgb(ACCENT_BRIGHT)
                                    },
                                ))
                                .child(div().flex_1().min_w_0().line_clamp(2).child(text)),
                        )
                    })
                    .children(status)
                    .when_some(hint, |s, d| {
                        s.child(
                            hint_pill(format!("⌘{d}"))
                                .absolute()
                                .top(px(6.))
                                .right(px(8.)),
                        )
                    })
                    .when(!meta.is_empty(), |s| {
                        s.child(
                            div()
                                .mt(px(2.))
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .text_size(px(10.5))
                                .text_color(sub)
                                .when(branch.is_some(), |s| {
                                    s.child(icon("git-branch").size(px(10.)).text_color(sub))
                                })
                                .child(
                                    div()
                                        .truncate()
                                        .font_family(terminal::FONT_FAMILY)
                                        .child(meta),
                                ),
                        )
                    })
                    .into_any_element()
            })
            .collect();

        div()
            .w(px(self.sidebar_width))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .relative()
            .bg(rgba(SIDEBAR_BG))
            .border_r_1()
            .border_color(rgb(BORDER))
            .child(
                // Titlebar row: traffic lights live on the left.
                div()
                    .h(px(TITLEBAR_H))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap_1()
                    .pr(px(10.))
                    .on_mouse_down(MouseButton::Left, |ev, window, _| {
                        titlebar::drag_or_zoom(ev, window)
                    })
                    .children(self.titlebar_buttons(cx)),
            )
            .child(
                div()
                    .id("workspaces")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .pb_2()
                    .children(rows),
            )
            .child(
                // Drag the right edge to resize; double-click resets.
                div()
                    .id("sidebar-edge")
                    .absolute()
                    .top_0()
                    .right_0()
                    .w(px(6.))
                    .h_full()
                    .cursor_col_resize()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                            if ev.click_count >= 2 {
                                this.sidebar_width = SIDEBAR_WIDTH;
                                cx.notify();
                            } else {
                                this.resizing_sidebar = true;
                            }
                            cx.stop_propagation();
                        }),
                    ),
            )
            .into_any_element()
    }

    /// Sidebar toggle and new-workspace buttons, shown in the sidebar's
    /// titlebar row or next to the traffic lights when it's hidden. Holding
    /// ⌘ swaps them for their shortcuts.
    fn titlebar_buttons(&self, cx: &mut Context<Self>) -> [AnyElement; 3] {
        let unread = self.notifications.iter().filter(|n| !n.read).count();
        let hints = self.hints == Some(Hints::Command);
        let button = |id: &'static str, name: &str, keys: &str| {
            if hints {
                div()
                    .id(id)
                    .flex()
                    .flex_none()
                    .items_center()
                    .h(px(22.))
                    .cursor_pointer()
                    .child(hint_pill(keys.into()))
            } else {
                icon_button(id, name)
            }
        };
        [
            button("toggle-sidebar", "panel-left", "⌘B")
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.sidebar = !this.sidebar;
                    cx.notify();
                }))
                .into_any_element(),
            button("notifications-button", "bell", "⌘I")
                .relative()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.notifications_open = !this.notifications_open;
                    cx.notify();
                }))
                .when(unread > 0 && !hints, |d| {
                    d.child(
                        div()
                            .absolute()
                            .top(px(-2.))
                            .right(px(-3.))
                            .min_w(px(13.))
                            .h(px(13.))
                            .px(px(3.))
                            .rounded_full()
                            .bg(rgb(ACCENT_BRIGHT))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(8.5))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(0xffffff))
                            .child(if unread > 99 {
                                "99+".to_string()
                            } else {
                                unread.to_string()
                            }),
                    )
                })
                .into_any_element(),
            button("new-ws", "plus", "⌘N")
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _, window, cx| this.new_workspace(None, window, cx)))
                .into_any_element(),
        ]
    }

    fn render_node(&self, node: &Node, multi: bool, cx: &mut Context<Self>) -> AnyElement {
        match node {
            Node::Leaf(p) => self.render_pane(p, multi, cx),
            Node::Split {
                id,
                axis,
                ratio,
                children,
            } => {
                let (id, axis) = (*id, *axis);
                let horizontal = axis == Axis::Horizontal;
                let bounds = self.split_bounds.clone();
                let divider = div()
                    .id(("divider", id))
                    .flex_none()
                    .bg(rgb(BORDER))
                    .when(horizontal, |s| s.w(px(1.)).h_full().cursor_col_resize())
                    .when(!horizontal, |s| s.h(px(1.)).w_full().cursor_row_resize())
                    .child(
                        // Wider invisible grab area centered on the 1px line.
                        div()
                            .absolute()
                            .when(horizontal, |s| s.left(px(-3.)).w(px(7.)).h_full())
                            .when(!horizontal, |s| s.top(px(-3.)).h(px(7.)).w_full()),
                    )
                    .relative()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.dragging = Some((id, axis));
                            cx.stop_propagation();
                        }),
                    );
                let a = div()
                    .flex_none()
                    .overflow_hidden()
                    .when(horizontal, |s| s.w(relative(*ratio)).h_full())
                    .when(!horizontal, |s| s.h(relative(*ratio)).w_full())
                    .child(self.render_node(&children[0], multi, cx));
                let b = div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .overflow_hidden()
                    .child(self.render_node(&children[1], multi, cx));
                div()
                    .size_full()
                    .flex()
                    .when(horizontal, |s| s.flex_row())
                    .when(!horizontal, |s| s.flex_col())
                    .relative()
                    .child(
                        canvas(
                            move |b, _, _| bounds.borrow_mut().insert(id, b),
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .child(a)
                    .child(divider)
                    .child(b)
                    .into_any_element()
            }
        }
    }

    fn render_pane(&self, pane: &Pane, multi: bool, cx: &mut Context<Self>) -> AnyElement {
        let pane_id = pane.id;
        let focused = self.ws().focused_pane == pane_id;
        let tab_hints = focused && self.hints == Some(Hints::Control);
        let tab_count = pane.tabs.len();
        let tabs = pane.tabs.iter().enumerate().map(|(ti, s)| {
            let active = ti == pane.active;
            let hint = shortcut_digit(ti, tab_count).filter(|_| tab_hints);
            let sid = s.entity_id();
            let unread = self.surface_unread(sid);
            let renaming = self
                .renaming_tab
                .as_ref()
                .filter(|(id, _)| *id == sid)
                .map(|(_, input)| input.clone());
            let group: SharedString = format!("tab-{sid}").into();
            div()
                .id(("tab", sid))
                .group(group.clone())
                .h_full()
                .max_w(px(220.))
                .min_w(px(80.))
                .px(px(10.))
                .flex()
                .items_center()
                .gap(px(6.))
                .border_r_1()
                .border_color(rgb(BORDER))
                .text_size(px(12.))
                .cursor_pointer()
                .when(active, |d| d.bg(rgb(PANE_BG)).text_color(rgb(TEXT)))
                .when(!active, |d| {
                    d.text_color(rgb(MUTED)).hover(|d| d.bg(rgba(HOVER)))
                })
                .when(active && focused && multi, |d| {
                    d.border_t_2().border_color(rgb(ACCENT))
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                        let ws = &mut this.workspaces[this.active];
                        ws.focused_pane = pane_id;
                        if let Some(p) = ws.root.pane_mut(pane_id) {
                            p.active = ti;
                        }
                        if ev.click_count >= 2 {
                            this.start_tab_rename(sid, window, cx);
                        } else {
                            this.focus_active(window, cx);
                        }
                        cx.notify();
                    }),
                )
                .on_mouse_down(
                    MouseButton::Middle,
                    cx.listener(move |this, _, window, cx| this.close_surface(sid, window, cx)),
                )
                .on_drag(
                    DraggedTab {
                        sid,
                        icon: s.icon(),
                        title: s.title(cx).into(),
                    },
                    |d, _, _, cx| cx.new(|_| d.clone()),
                )
                .drag_over::<DraggedTab>(|d, _, _, _| d.bg(rgba(DROP_TINT)))
                .on_drop(cx.listener(move |this, d: &DraggedTab, window, cx| {
                    this.move_tab(
                        d.sid,
                        TabDrop::Tabs {
                            pane: pane_id,
                            index: ti,
                        },
                        window,
                        cx,
                    )
                }))
                .when(unread, |d| {
                    d.text_color(rgb(TEXT)).child(
                        div()
                            .size(px(6.))
                            .flex_none()
                            .rounded_full()
                            .bg(rgb(ACCENT_BRIGHT)),
                    )
                })
                .child(
                    icon(s.icon())
                        .size(px(12.))
                        .text_color(if active || unread {
                            rgb(TEXT)
                        } else {
                            rgb(MUTED)
                        }),
                )
                .child(match renaming.clone() {
                    Some(input) => div()
                        .flex_1()
                        .min_w(px(60.))
                        .h(px(20.))
                        .px(px(4.))
                        .flex()
                        .items_center()
                        .rounded(px(4.))
                        .bg(rgba(0x0000004d))
                        .border_1()
                        .border_color(rgb(ACCENT))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                            if ev.keystroke.key == "escape" {
                                this.renaming_tab = None;
                                this.focus_active(window, cx);
                                cx.notify();
                            }
                        }))
                        .child(
                            Input::new(&input)
                                .appearance(false)
                                .text_size(px(12.))
                                .h_full()
                                .px_0()
                                .py_0(),
                        )
                        .into_any_element(),
                    None => div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(s.title(cx))
                        .into_any_element(),
                })
                .when_some(hint, |d, n| d.child(hint_pill(format!("⌃{n}"))))
                .when(hint.is_none() && renaming.is_none(), |d| {
                    d.child(
                        div()
                            .id(("tab-close", sid))
                            .flex_none()
                            .rounded(px(3.))
                            .p(px(2.))
                            .when(!active, |d| {
                                d.invisible().group_hover(group.clone(), |d| d.visible())
                            })
                            .hover(|d| d.bg(rgba(0xffffff18)))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.close_surface(sid, window, cx)
                            }))
                            .child(icon("x").size(px(10.)).text_color(rgb(MUTED))),
                    )
                })
        });

        let focus_pane = move |this: &mut Self, cx: &mut Context<Self>| {
            this.workspaces[this.active].focused_pane = pane_id;
            cx.notify();
        };
        let tab_bar =
            div()
                .id(("tabbar", pane_id))
                .h(px(TABBAR_H))
                .flex_none()
                .flex()
                .items_center()
                .bg(rgb(TABBAR_BG))
                .border_b_1()
                .border_color(rgb(BORDER))
                .child(
                    div()
                        .id(("tabs", pane_id))
                        .h_full()
                        .flex()
                        .overflow_x_scroll()
                        .children(tabs),
                )
                .child(
                    div()
                        .flex_1()
                        .h_full()
                        .on_mouse_down(MouseButton::Left, |ev, window, _| {
                            titlebar::drag_or_zoom(ev, window)
                        })
                        .drag_over::<DraggedTab>(|d, _, _, _| d.bg(rgba(DROP_TINT)))
                        .on_drop(cx.listener(move |this, d: &DraggedTab, window, cx| {
                            this.move_tab(
                                d.sid,
                                TabDrop::Tabs {
                                    pane: pane_id,
                                    index: tab_count,
                                },
                                window,
                                cx,
                            )
                        })),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(2.))
                        .px(px(6.))
                        // Shown while this pane is zoomed (⌘⇧↩); a click unzooms.
                        .when(self.ws().zoom == Some(pane_id), |d| {
                            d.child(
                                tinted_icon_button(("pane-zoom", pane_id), "maximize-2", ACCENT)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.workspaces[this.active].toggle_zoom();
                                        cx.notify();
                                    })),
                            )
                        })
                        .child(icon_button(("pane-term", pane_id), "terminal").on_click(
                            cx.listener(move |this, _, window, cx| {
                                focus_pane(this, cx);
                                let cwd = this.focused_cwd(cx);
                                let ws = this.ws().id;
                                let s = this.make_terminal(cwd, ws, window, cx);
                                this.add_tab(s, window, cx);
                            }),
                        ))
                        .child(
                            icon_button(("pane-web", pane_id), "globe").on_click(cx.listener(
                                move |this, _, window, cx| {
                                    focus_pane(this, cx);
                                    let s = this.make_browser(DEFAULT_URL, window, cx);
                                    this.add_tab(s, window, cx);
                                },
                            )),
                        )
                        .child(
                            icon_button(("pane-split-r", pane_id), "split-right").on_click(
                                cx.listener(move |this, _, window, cx| {
                                    focus_pane(this, cx);
                                    this.split(Axis::Horizontal, false, window, cx);
                                }),
                            ),
                        )
                        .child(
                            icon_button(("pane-split-d", pane_id), "split-down").on_click(
                                cx.listener(move |this, _, window, cx| {
                                    focus_pane(this, cx);
                                    this.split(Axis::Vertical, false, window, cx);
                                }),
                            ),
                        ),
                );

        let content = match pane.active_surface() {
            Some(Surface::Terminal(t)) => t.clone().into_any_element(),
            Some(Surface::Browser(b)) => b.clone().into_any_element(),
            None => div().into_any_element(),
        };

        // A ring around panes with unread notifications, like cmux.
        let ring = pane.tabs.iter().any(|s| self.surface_unread(s.entity_id()));
        div()
            .id(("pane", pane_id))
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(PANE_BG))
            .relative()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| focus_pane(this, cx)),
            )
            .child(tab_bar)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(content)
                    // Unfocused splits are dimmed, like cmux/ghostty.
                    .when(multi && !focused, |d| {
                        d.child(div().absolute().inset_0().bg(rgba(0x0000004a)))
                    })
                    // Catches a dragged tab over the content (terminals and
                    // webviews would otherwise swallow it).
                    .when(cx.has_active_drag(), |d| {
                        d.child(
                            div()
                                .id(("drop", pane_id))
                                .absolute()
                                .inset_0()
                                .on_drag_move(cx.listener(
                                    move |this, ev: &DragMoveEvent<DraggedTab>, _, cx| {
                                        this.drag_over_pane(pane_id, ev, cx)
                                    },
                                ))
                                .on_drop(cx.listener(|this, d: &DraggedTab, window, cx| {
                                    match this.tab_drop {
                                        Some((_, to, _)) => this.move_tab(d.sid, to, window, cx),
                                        None => cx.notify(),
                                    }
                                })),
                        )
                    })
                    .when_some(self.flash.filter(|(p, _)| *p == pane_id), |d, (_, n)| {
                        d.child(
                            div()
                                .absolute()
                                .inset_0()
                                .border_2()
                                .border_color(rgb(ACCENT))
                                .bg(rgba(0x1f6fe522))
                                .with_animation(
                                    ("flash", n),
                                    Animation::new(Duration::from_millis(600)),
                                    |d, t| d.opacity(1. - t),
                                ),
                        )
                    }),
            )
            .when(ring, |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .border_2()
                        .border_color(rgb(ACCENT_BRIGHT)),
                )
            })
            // Which split has the keyboard, at a glance.
            .when(multi && focused && !ring, |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .border_1()
                        .border_color(rgba(ACCENT << 8 | 0xb0)),
                )
            })
            .into_any_element()
    }
}

impl Render for AbroMux {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.window_bounds = Some(window.window_bounds().get_bounds());
        self.mark_viewed(window);
        for ws in &mut self.workspaces {
            ws.zoomed();
        }
        if !cx.has_active_drag() {
            self.tab_drop = None;
        }
        self.sync_webviews(cx);
        let notifications = self
            .notifications_open
            .then(|| self.render_notifications(cx));
        let palette = self.render_palette(cx);
        let ws = self.ws();
        let multi = ws.root.panes().len() > 1;
        let main = match ws
            .zoom
            .and_then(|z| ws.root.panes().into_iter().find(|p| p.id == z))
        {
            Some(p) => self.render_pane(p, multi, cx),
            None => self.render_node(&ws.root, multi, cx),
        };
        let sidebar = if self.sidebar {
            Some(self.render_sidebar(cx))
        } else {
            None
        };
        let title = self.ws().name(cx);

        div()
            .id("abromux")
            .key_context("AbroMux")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .text_color(rgb(TEXT))
            .font_family(".AppleSystemUIFont")
            .on_action(cx.listener(|_, _: &Quit, window, cx| quit::request(window, cx)))
            .on_action(cx.listener(|this, _: &NewWorkspace, window, cx| {
                let cwd = this.focused_cwd(cx);
                this.new_workspace(cwd, window, cx)
            }))
            .on_action(cx.listener(|this, _: &CloseWorkspace, window, cx| {
                this.close_workspace(this.active, window, cx)
            }))
            .on_action(cx.listener(|this, _: &RenameWorkspace, window, cx| {
                this.sidebar = true;
                this.start_rename(this.active, window, cx)
            }))
            .on_action(cx.listener(|this, _: &NextWorkspace, window, cx| {
                let n = this.workspaces.len();
                this.select_workspace((this.active + 1) % n, window, cx)
            }))
            .on_action(cx.listener(|this, _: &PrevWorkspace, window, cx| {
                let n = this.workspaces.len();
                this.select_workspace((this.active + n - 1) % n, window, cx)
            }))
            .on_action(cx.listener(|this, _: &NewTerminal, window, cx| {
                let cwd = this.focused_cwd(cx);
                let ws = this.ws().id;
                let s = this.make_terminal(cwd, ws, window, cx);
                this.add_tab(s, window, cx)
            }))
            .on_action(cx.listener(|this, _: &NewBrowser, window, cx| {
                let s = this.make_browser(DEFAULT_URL, window, cx);
                this.add_tab(s, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SplitRight, window, cx| {
                this.split(Axis::Horizontal, false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SplitDown, window, cx| {
                this.split(Axis::Vertical, false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                if let Some(id) = this.ws().focused_surface().map(|s| s.entity_id()) {
                    this.close_surface(id, window, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &NextTab, window, cx| this.cycle_tab(1, window, cx)))
            .on_action(cx.listener(|this, _: &PrevTab, window, cx| this.cycle_tab(-1, window, cx)))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
                this.sidebar = !this.sidebar;
                cx.notify()
            }))
            .on_action(cx.listener(|this, _: &FocusAddressBar, window, cx| {
                if let Some(Surface::Browser(b)) = this.ws().focused_surface() {
                    BrowserView::focus_address(b, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Ws1, w, cx| this.select_workspace(0, w, cx)))
            .on_action(cx.listener(|this, _: &Ws2, w, cx| this.select_workspace(1, w, cx)))
            .on_action(cx.listener(|this, _: &Ws3, w, cx| this.select_workspace(2, w, cx)))
            .on_action(cx.listener(|this, _: &Ws4, w, cx| this.select_workspace(3, w, cx)))
            .on_action(cx.listener(|this, _: &Ws5, w, cx| this.select_workspace(4, w, cx)))
            .on_action(cx.listener(|this, _: &Ws6, w, cx| this.select_workspace(5, w, cx)))
            .on_action(cx.listener(|this, _: &Ws7, w, cx| this.select_workspace(6, w, cx)))
            .on_action(cx.listener(|this, _: &Ws8, w, cx| this.select_workspace(7, w, cx)))
            .on_action(cx.listener(|this, _: &Ws9, w, cx| {
                let last = this.workspaces.len() - 1;
                this.select_workspace(last, w, cx)
            }))
            .on_action(cx.listener(|this, _: &Tab1, w, cx| this.select_tab(Some(0), w, cx)))
            .on_action(cx.listener(|this, _: &Tab2, w, cx| this.select_tab(Some(1), w, cx)))
            .on_action(cx.listener(|this, _: &Tab3, w, cx| this.select_tab(Some(2), w, cx)))
            .on_action(cx.listener(|this, _: &Tab4, w, cx| this.select_tab(Some(3), w, cx)))
            .on_action(cx.listener(|this, _: &Tab5, w, cx| this.select_tab(Some(4), w, cx)))
            .on_action(cx.listener(|this, _: &Tab6, w, cx| this.select_tab(Some(5), w, cx)))
            .on_action(cx.listener(|this, _: &Tab7, w, cx| this.select_tab(Some(6), w, cx)))
            .on_action(cx.listener(|this, _: &Tab8, w, cx| this.select_tab(Some(7), w, cx)))
            .on_action(cx.listener(|this, _: &Tab9, w, cx| this.select_tab(None, w, cx)))
            .on_action(cx.listener(|this, _: &FocusLeft, w, cx| this.focus_dir(Dir::Left, w, cx)))
            .on_action(cx.listener(|this, _: &FocusRight, w, cx| this.focus_dir(Dir::Right, w, cx)))
            .on_action(cx.listener(|this, _: &FocusUp, w, cx| this.focus_dir(Dir::Up, w, cx)))
            .on_action(cx.listener(|this, _: &FocusDown, w, cx| this.focus_dir(Dir::Down, w, cx)))
            .on_action(
                cx.listener(|this, _: &ResizeLeft, _, cx| this.resize_focused(Dir::Left, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ResizeRight, _, cx| this.resize_focused(Dir::Right, cx)),
            )
            .on_action(cx.listener(|this, _: &ResizeUp, _, cx| this.resize_focused(Dir::Up, cx)))
            .on_action(
                cx.listener(|this, _: &ResizeDown, _, cx| this.resize_focused(Dir::Down, cx)),
            )
            .on_action(cx.listener(|this, _: &FlashPane, _, cx| this.flash_pane(cx)))
            .on_action(cx.listener(|this, _: &ToggleZoom, _, cx| {
                if this.workspaces[this.active].toggle_zoom() {
                    cx.notify()
                }
            }))
            .on_action(cx.listener(|this, _: &RenameTab, window, cx| {
                if let Some(id) = this.ws().focused_surface().map(|s| s.entity_id()) {
                    this.start_tab_rename(id, window, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleNotifications, window, cx| {
                this.close_palette(window, cx);
                this.notifications_open = !this.notifications_open;
                cx.notify()
            }))
            .on_action(cx.listener(|this, _: &CommandPalette, window, cx| {
                this.toggle_palette(palette::Mode::Commands, window, cx)
            }))
            .on_action(cx.listener(|this, _: &WorkspaceSwitcher, window, cx| {
                this.toggle_palette(palette::Mode::Workspaces, window, cx)
            }))
            .map(|d| self.palette_actions(d, cx))
            .on_action(cx.listener(|this, _: &JumpToUnread, window, cx| {
                this.jump_to_unread(window, cx);
            }))
            .when(self.notifications_open, |d| {
                d.capture_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                    if ev.keystroke.key == "escape" {
                        this.notifications_open = false;
                        cx.stop_propagation();
                        cx.notify();
                    }
                }))
            })
            .on_modifiers_changed(cx.listener(|this, ev: &ModifiersChangedEvent, w, cx| {
                this.modifiers_changed(&ev.modifiers, w, cx)
            }))
            // Pressing a key while holding ⌘/⌃ means a shortcut, not a look at the hints.
            .capture_key_down(cx.listener(|this, _: &KeyDownEvent, _, cx| this.set_hints(None, cx)))
            .when(self.resizing_sidebar, |d| d.cursor_col_resize())
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                if this.resizing_sidebar {
                    if ev.pressed_button != Some(MouseButton::Left) {
                        this.resizing_sidebar = false;
                    } else {
                        this.sidebar_width =
                            f32::from(ev.position.x).clamp(SIDEBAR_MIN, SIDEBAR_MAX);
                        cx.notify();
                    }
                    return;
                }
                let Some((id, axis)) = this.dragging else {
                    return;
                };
                if ev.pressed_button != Some(MouseButton::Left) {
                    this.dragging = None;
                    return;
                }
                let Some(b) = this.split_bounds.borrow().get(&id).copied() else {
                    return;
                };
                let r = if axis == Axis::Horizontal {
                    (ev.position.x - b.origin.x) / b.size.width
                } else {
                    (ev.position.y - b.origin.y) / b.size.height
                };
                let ws = &mut this.workspaces[this.active];
                if let Some(ratio) = ws.root.split_mut(id) {
                    *ratio = r.clamp(0.1, 0.9);
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    this.dragging = None;
                    this.resizing_sidebar = false;
                }),
            )
            .children(sidebar)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .bg(rgb(PANE_BG))
                    .when(!self.sidebar, |d| {
                        // Same height as the sidebar's titlebar row so the
                        // traffic lights sit centered in it.
                        d.child(
                            div()
                                .h(px(TITLEBAR_H))
                                .flex_none()
                                .flex()
                                .items_center()
                                .pl(px(80.))
                                .pr(px(10.))
                                .gap_1()
                                .bg(rgb(TABBAR_BG))
                                .border_b_1()
                                .border_color(rgb(BORDER))
                                .on_mouse_down(MouseButton::Left, |ev, window, _| {
                                    titlebar::drag_or_zoom(ev, window)
                                })
                                .children(self.titlebar_buttons(cx))
                                .child(
                                    div()
                                        .ml_2()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(px(12.))
                                        .text_color(rgb(MUTED))
                                        .child(title),
                                ),
                        )
                    })
                    .child({
                        let (bounds, origin) =
                            (self.main_bounds.clone(), self.main_bounds.get().origin);
                        div()
                            .flex_1()
                            .min_h_0()
                            .relative()
                            .child(
                                canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {})
                                    .absolute()
                                    .size_full(),
                            )
                            .child(main)
                            .when_some(self.tab_drop, |d, (_, _, r)| {
                                d.child(
                                    div()
                                        .absolute()
                                        .left(r.origin.x - origin.x)
                                        .top(r.origin.y - origin.y)
                                        .w(r.size.width)
                                        .h(r.size.height)
                                        .rounded(px(4.))
                                        .border_2()
                                        .border_color(rgb(ACCENT))
                                        .bg(rgba(DROP_TINT)),
                                )
                            })
                    }),
            )
            .relative()
            .children(notifications)
            .children(palette)
    }
}

/// A tab being dragged, drawn as a chip under the cursor.
#[derive(Clone)]
struct DraggedTab {
    sid: EntityId,
    icon: &'static str,
    title: SharedString,
}

impl Render for DraggedTab {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .h(px(TABBAR_H))
            .max_w(px(220.))
            .px(px(10.))
            .flex()
            .items_center()
            .gap(px(6.))
            .rounded(px(5.))
            .border_1()
            .border_color(rgb(ACCENT))
            .bg(rgb(PANE_BG))
            .opacity(0.9)
            .text_size(px(12.))
            .text_color(rgb(TEXT))
            .font_family(".AppleSystemUIFont")
            .child(icon(self.icon).size(px(12.)).text_color(rgb(TEXT)))
            .child(div().min_w_0().truncate().child(self.title.clone()))
    }
}

/// A name typed into a single-line field: a newline that slipped in would
/// panic gpui's line shaping wherever the name is drawn.
fn one_line(s: &str) -> String {
    s.split(['\n', '\r'])
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The ⌘/⌃ digit that selects item `index` of `count`: 1–8 for the first
/// eight, 9 for the last.
fn shortcut_digit(index: usize, count: usize) -> Option<usize> {
    if index < 8 {
        Some(index + 1)
    } else if index + 1 == count {
        Some(9)
    } else {
        None
    }
}

/// A small rounded label like cmux's shortcut hint pill.
fn hint_pill(text: String) -> Div {
    div()
        .flex_none()
        .px(px(6.))
        .py(px(1.))
        .rounded_full()
        .bg(rgba(0x3a3c42f2))
        .border_1()
        .border_color(rgba(0xffffff4d))
        .shadow_sm()
        .text_size(px(10.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(TEXT))
        .child(text)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "bench-vt") {
        std::process::exit(bench::vt(&args[1..]));
    }
    if !args.is_empty() {
        std::process::exit(cli::run(args));
    }
    if let Some(code) = cli::launch() {
        std::process::exit(code);
    }
    run_app(std::env::var("ABROMUX_CWD").ok());
}

fn run_app(cwd: Option<String>) {
    Application::new().with_assets(Assets).run(|cx: &mut App| {
        icon::set_dock_icon();
        gpui_component::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        let theme = Theme::global_mut(cx);
        theme.background = rgba(0x00000000).into();
        theme.font_family = ".AppleSystemUIFont".into();
        theme.mono_font_family = terminal::FONT_FAMILY.into();

        cx.set_global(config::Config::load());
        cx.on_action(|_: &Quit, cx| {
            match cx.active_window().or_else(|| cx.windows().first().copied()) {
                Some(w) => w
                    .update(cx, |_, window, cx| quit::request(window, cx))
                    .unwrap_or_else(|_| cx.quit()),
                None => cx.quit(),
            }
        });
        cx.bind_keys([
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-n", NewWorkspace, None),
            KeyBinding::new("cmd-shift-w", CloseWorkspace, None),
            KeyBinding::new("cmd-shift-r", RenameWorkspace, None),
            KeyBinding::new("ctrl-cmd-]", NextWorkspace, None),
            KeyBinding::new("ctrl-cmd-[", PrevWorkspace, None),
            KeyBinding::new("cmd-t", NewTerminal, None),
            KeyBinding::new("cmd-shift-l", NewBrowser, None),
            KeyBinding::new("cmd-d", SplitRight, None),
            KeyBinding::new("cmd-shift-d", SplitDown, None),
            KeyBinding::new("cmd-w", CloseTab, None),
            // ⌘⇧] arrives from macOS as cmd-} (shift folded into the key), so
            // "cmd-shift-]" would never match.
            KeyBinding::new("cmd-}", NextTab, None),
            KeyBinding::new("cmd-{", PrevTab, None),
            KeyBinding::new("cmd-b", ToggleSidebar, None),
            KeyBinding::new("cmd-l", FocusAddressBar, None),
            KeyBinding::new("cmd-1", Ws1, None),
            KeyBinding::new("cmd-2", Ws2, None),
            KeyBinding::new("cmd-3", Ws3, None),
            KeyBinding::new("cmd-4", Ws4, None),
            KeyBinding::new("cmd-5", Ws5, None),
            KeyBinding::new("cmd-6", Ws6, None),
            KeyBinding::new("cmd-7", Ws7, None),
            KeyBinding::new("cmd-8", Ws8, None),
            KeyBinding::new("cmd-9", Ws9, None),
            KeyBinding::new("ctrl-1", Tab1, None),
            KeyBinding::new("ctrl-2", Tab2, None),
            KeyBinding::new("ctrl-3", Tab3, None),
            KeyBinding::new("ctrl-4", Tab4, None),
            KeyBinding::new("ctrl-5", Tab5, None),
            KeyBinding::new("ctrl-6", Tab6, None),
            KeyBinding::new("ctrl-7", Tab7, None),
            KeyBinding::new("ctrl-8", Tab8, None),
            KeyBinding::new("ctrl-9", Tab9, None),
            KeyBinding::new("ctrl-tab", NextTab, None),
            KeyBinding::new("ctrl-shift-tab", PrevTab, None),
            KeyBinding::new("alt-cmd-left", FocusLeft, None),
            KeyBinding::new("alt-cmd-right", FocusRight, None),
            KeyBinding::new("alt-cmd-up", FocusUp, None),
            KeyBinding::new("alt-cmd-down", FocusDown, None),
            KeyBinding::new("ctrl-shift-h", ResizeLeft, None),
            KeyBinding::new("ctrl-shift-j", ResizeDown, None),
            KeyBinding::new("ctrl-shift-k", ResizeUp, None),
            KeyBinding::new("ctrl-shift-l", ResizeRight, None),
            KeyBinding::new("cmd-shift-h", FlashPane, None),
            KeyBinding::new("cmd-shift-enter", ToggleZoom, None),
            KeyBinding::new("cmd-i", ToggleNotifications, None),
            KeyBinding::new("cmd-shift-u", JumpToUnread, None),
            KeyBinding::new("cmd-shift-p", CommandPalette, None),
            KeyBinding::new("cmd-p", WorkspaceSwitcher, None),
            KeyBinding::new("down", palette::SelectNext, Some("CommandPalette > Input")),
            KeyBinding::new(
                "ctrl-n",
                palette::SelectNext,
                Some("CommandPalette > Input"),
            ),
            KeyBinding::new("up", palette::SelectPrev, Some("CommandPalette > Input")),
            KeyBinding::new(
                "ctrl-p",
                palette::SelectPrev,
                Some("CommandPalette > Input"),
            ),
            KeyBinding::new("enter", palette::Confirm, Some("CommandPalette > Input")),
            KeyBinding::new("escape", palette::Dismiss, Some("CommandPalette > Input")),
            // Keybindings run before key listeners; let these reach the PTY.
            KeyBinding::new("tab", NoAction, Some("Terminal")),
            KeyBinding::new("shift-tab", NoAction, Some("Terminal")),
            KeyBinding::new("ctrl-shift-i", NoAction, Some("Terminal")),
            KeyBinding::new("cmd-c", terminal::Copy, Some("Terminal")),
            KeyBinding::new("cmd-v", terminal::Paste, Some("Terminal")),
            KeyBinding::new("cmd-=", terminal::IncreaseFontSize, Some("Terminal")),
            KeyBinding::new("cmd-+", terminal::IncreaseFontSize, Some("Terminal")),
            KeyBinding::new("cmd--", terminal::DecreaseFontSize, Some("Terminal")),
            KeyBinding::new("cmd-0", terminal::ResetFontSize, Some("Terminal")),
            KeyBinding::new("cmd-k", terminal::ClearScrollback, Some("Terminal")),
            KeyBinding::new("cmd-f", terminal::Find, Some("Terminal")),
            KeyBinding::new("cmd-g", terminal::FindNext, Some("Terminal")),
            KeyBinding::new("cmd-shift-g", terminal::FindPrevious, Some("Terminal")),
            KeyBinding::new("alt-cmd-g", terminal::FindPrevious, Some("Terminal")),
            KeyBinding::new("cmd-e", terminal::UseSelectionForFind, Some("Terminal")),
            KeyBinding::new("shift-enter", terminal::FindPrevious, Some("FindBar")),
            KeyBinding::new("cmd-[", browser::Back, Some("Browser")),
            KeyBinding::new("cmd-]", browser::Forward, Some("Browser")),
            KeyBinding::new("cmd-r", browser::Reload, Some("Browser")),
            KeyBinding::new("alt-cmd-i", browser::ToggleDevTools, Some("Browser")),
            KeyBinding::new("alt-cmd-c", browser::ShowConsole, Some("Browser")),
            KeyBinding::new("cmd-f", terminal::Find, Some("Browser")),
            KeyBinding::new("cmd-g", terminal::FindNext, Some("Browser")),
            KeyBinding::new("cmd-shift-g", terminal::FindPrevious, Some("Browser")),
            KeyBinding::new("alt-cmd-g", terminal::FindPrevious, Some("Browser")),
            KeyBinding::new("cmd-e", terminal::UseSelectionForFind, Some("Browser")),
            // The address and find fields take these for search and indent.
            KeyBinding::new("cmd-f", terminal::Find, Some("Browser > Input")),
            KeyBinding::new("cmd-[", browser::Back, Some("Browser > Input")),
            KeyBinding::new("cmd-]", browser::Forward, Some("Browser > Input")),
        ]);
        // The Edit items send copy:/paste:/… down the responder chain, which is
        // the only way the browser's WKWebView gets ⌘C/⌘V/⌘A/⌘X. gpui views
        // handle those keys through their own bindings before the menu sees them.
        cx.set_menus(vec![
            Menu {
                name: channel::app_name().into(),
                items: vec![MenuItem::action(
                    format!("Quit {}", channel::app_name()),
                    Quit,
                )],
            },
            Menu {
                name: "Edit".into(),
                items: vec![
                    MenuItem::os_action("Cut", gpui_component::input::Cut, OsAction::Cut),
                    MenuItem::os_action("Copy", terminal::Copy, OsAction::Copy),
                    MenuItem::os_action("Paste", terminal::Paste, OsAction::Paste),
                    MenuItem::os_action(
                        "Select All",
                        gpui_component::input::SelectAll,
                        OsAction::SelectAll,
                    ),
                ],
            },
        ]);
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let session = session::load();
        let bounds = session
            .as_ref()
            .and_then(|s| s.window)
            .map(|w| w.bounds())
            .unwrap_or_else(|| Bounds::centered(None, size(px(1280.), px(820.)), cx));
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(640.), px(400.))),
                titlebar: Some(TitlebarOptions {
                    title: Some(channel::app_name().into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(14.), px(13.))),
                }),
                window_background: WindowBackgroundAppearance::Blurred,
                // Only empty titlebar areas move it (titlebar.rs), not tab drags.
                is_movable: false,
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| AbroMux::new(cwd, session, window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            },
        )
        .expect("open window");
        cx.activate(true);
    });
}
