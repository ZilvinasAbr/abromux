//! The command palette (⌘⇧P) and the switcher (⌘P), as in cmux.
//!
//! ⌘P has two columns: every workspace on the left, the selected
//! workspace's tabs on the right (pinned ones first, hidden ones too, then
//! its other tabs and its suggested links). What's typed filters both;
//! Tab (or ⇧Tab) moves between them, ↑ ↓ pick within one, Enter goes there.
//!
//! Commands are the app's actions with a title. Only the ones that apply to
//! what had focus when the palette opened are listed (terminal commands for
//! a terminal, browser ones for a browser), each with its shortcut. Choosing
//! one gives focus back and dispatches the action there, exactly as its
//! shortcut would.

use crate::{
    AbroMux, TITLEBAR_H, channel, keymap,
    settings::{self, Command, Settings},
    shortcut_digit,
    theme::*,
    workspace::Surface,
    workspace::tilde,
};
use gpui::{prelude::*, *};
use gpui_component::input::{Input, InputEvent, InputState};
use std::path::Path;

actions!(
    palette,
    [SelectNext, SelectPrev, Confirm, Dismiss, SwitchColumn]
);

/// Key context of the palette, for its bindings in main.rs.
pub const CONTEXT: &str = "CommandPalette";
const WIDTH: f32 = 560.;
/// ⌘P's width, for its two columns.
const SWITCHER_WIDTH: f32 = 900.;
const MAX_ROWS_HEIGHT: f32 = 360.;

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    Commands,
    Workspaces,
    /// A plugin's "Open … link": paste a link it opens. No rows; Enter
    /// opens it in a new tab.
    OpenLink(&'static str),
    /// ⇧⌘M: the cached GitLab merge requests. A pasted link or
    /// `group/project!12` that isn't cached opens too.
    MergeRequests,
    /// A question a script asked (`ui.prompt`): a line of text, or a
    /// choice from its items.
    Prompt,
}

/// The question `Mode::Prompt` shows, and where its answer goes.
pub(crate) struct Prompt {
    pub id: u64,
    pub title: String,
    /// A list to choose from, not a line of text.
    pub pick: bool,
    /// Pick: typed text that matches no item answers too.
    pub allow_other: bool,
    /// What Enter does, said under the field.
    pub confirm: Option<String>,
    /// Dropped unanswered (Esc, a click outside, another palette), the
    /// asker hears it was cancelled.
    pub answer: Option<futures::channel::oneshot::Sender<serde_json::Value>>,
}

/// One item of a pick: id, title, subtitle, detail.
pub(crate) type PromptItem = (String, String, Option<String>, Option<String>);

enum Target {
    Action(Box<dyn Action>),
    Workspace(usize),
    /// A tab, hidden or not, in any workspace.
    Tab(EntityId),
    /// A suggested link of workspace `.0` (by id), opened in a pinned tab.
    Link(usize, crate::links::Link),
    Mr(crate::gitlab::model::MrKey),
    /// A prompt's item, by id.
    Choice(String),
}

struct Entry {
    title: SharedString,
    subtitle: Option<SharedString>,
    /// More words it matches, after the title.
    keywords: Vec<String>,
    keys: Option<SharedString>,
    target: Target,
    /// Drawn left of the title: an icon's name, and its colour if not the
    /// text's.
    icon: Option<(&'static str, Option<u32>)>,
    /// The workspace or tab you're on now.
    current: bool,
}

/// ⌘P's tabs column: its own rows, matches and selection.
struct Column {
    entries: Vec<Entry>,
    matches: Vec<(usize, Vec<usize>)>,
    selected: usize,
    scroll: ScrollHandle,
}

pub struct Palette {
    pub mode: Mode,
    input: Entity<InputState>,
    entries: Vec<Entry>,
    /// Indexes into `entries` that match the query, best first, with the
    /// matched character positions of each title.
    matches: Vec<(usize, Vec<usize>)>,
    selected: usize,
    /// Where focus goes back to, and where commands are dispatched.
    prev_focus: Option<FocusHandle>,
    scroll: ScrollHandle,
    /// Workspaces (⌘P): the selected workspace's tabs, beside the
    /// workspaces.
    tabs: Option<Column>,
    /// Workspaces: the tabs column has the selection (Tab switches).
    on_tabs: bool,
    /// Prompt: the question.
    pub(crate) prompt: Option<Prompt>,
    /// What `filter` last matched against.
    query: String,
}

/// A palette command: its title, subtitle, extra search words and action.
type Cmd = (String, Option<String>, Vec<String>, Box<dyn Action>);

/// Commands in palette order: the actions with a title, then the config
/// file's commands, then those of the project file at `project` (which
/// replace config ones of the same title). Ws1–9 and Tab1–9 have none: the
/// switcher and the tab bar cover them.
fn commands(project: Option<&Path>, cx: &App) -> Vec<Cmd> {
    // GitLab turned off (`gitlab.enabled`): none of its commands.
    let gitlab = crate::plugins::enabled("gitlab", cx);
    let builtin = keymap::ACTIONS
        .iter()
        .filter(|a| {
            gitlab
                || !matches!(
                    a.name,
                    "toggleMergeRequests" | "goToMergeRequest" | "refreshMergeRequests"
                )
        })
        .filter_map(|a| a.title.map(|t| (t.to_string(), None, vec![], (a.action)())));
    let plugins = crate::plugins::all()
        .iter()
        .filter(|p| crate::plugins::enabled(p.id, cx))
        .flat_map(|p| -> Vec<Cmd> {
            let action = Box::new(keymap::OpenPlugin { id: p.id.into() });
            let mut cmds: Vec<Cmd> = vec![(p.title.into(), None, vec![p.id.into()], action)];
            if let Some(title) = p.open_link {
                let action = Box::new(keymap::OpenPluginLink { id: p.id.into() });
                cmds.push((
                    title.into(),
                    None,
                    vec![p.id.into(), "url".into(), "link".into(), "paste".into()],
                    action,
                ));
            }
            cmds
        });
    // Each server of Settings › Servers: a workspace or a tab there.
    let servers = crate::servers::load()
        .into_iter()
        .flat_map(|s| -> Vec<Cmd> {
            let words = || {
                vec![
                    "server".into(),
                    "remote".into(),
                    "ssh".into(),
                    s.address.clone(),
                ]
            };
            let on = |tab| -> Box<dyn Action> {
                Box::new(keymap::OnServer {
                    address: s.address.clone(),
                    tab,
                })
            };
            vec![
                (
                    format!("Open Workspaces from {}", s.name),
                    Some("its workspaces that aren't open here".into()),
                    words(),
                    Box::new(keymap::OpenFromServer {
                        address: s.address.clone(),
                    }),
                ),
                (
                    format!("New Workspace on {}", s.name),
                    Some(s.address.clone()),
                    words(),
                    on(false),
                ),
                (
                    format!("New Tab on {}", s.name),
                    Some(s.address.clone()),
                    words(),
                    on(true),
                ),
            ]
        });
    let builtin = builtin.chain(plugins).chain(servers);
    // A project file that doesn't parse shows its error as a command.
    let local = project.map(|p| (p, settings::read_project(p)));
    let local_titles: Vec<String> = match &local {
        Some((_, Ok(cmds))) => cmds.iter().map(|(c, _)| c.title.clone()).collect(),
        _ => vec![],
    };
    let entry = |c: &Command, project: Option<&Path>, index| -> Cmd {
        let action: Box<dyn Action> = Box::new(keymap::RunCommand {
            index,
            project: project.map(Path::to_path_buf),
        });
        let subtitle = c.description.clone().or_else(|| {
            let root = settings::project_root(project?);
            let name = root.file_name().unwrap_or(root.as_os_str());
            Some(format!("{} project", name.to_string_lossy()))
        });
        (c.title.clone(), subtitle, c.keywords.clone(), action)
    };
    let global: Vec<Cmd> = cx
        .global::<Settings>()
        .commands
        .iter()
        .enumerate()
        .filter(|(_, c)| !local_titles.contains(&c.title))
        .map(|(i, c)| entry(c, None, i))
        .collect();
    let local: Vec<Cmd> = match &local {
        Some((p, Ok(cmds))) => cmds
            .iter()
            .enumerate()
            .map(|(i, (c, _))| entry(c, Some(p), i))
            .collect(),
        Some((_, Err(e))) => vec![(
            "Project commands not loaded".into(),
            Some(e.clone()),
            vec![],
            Box::new(crate::ReloadConfig),
        )],
        None => vec![],
    };
    builtin.chain(global).chain(local).collect()
}

/// Every cached merge request, recently opened and yours first.
fn mr_entries(cx: &App) -> Vec<Entry> {
    let st = crate::gitlab::store(cx).read(cx);
    let mut mrs: Vec<_> = st.mrs.values().collect();
    mrs.sort_by(|a, b| {
        (b.inspected_at, b.roles.any(), &b.key).cmp(&(a.inspected_at, a.roles.any(), &a.key))
    });
    mrs.into_iter()
        .map(|m| {
            let l = m.light.as_ref();
            let title = l
                .map(|l| l.title.clone())
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| m.key.reference());
            let mut sub = vec![m.key.reference()];
            if let Some(l) = l {
                sub.push(l.source_branch.clone());
                if !l.author.is_empty() {
                    sub.push(l.author.clone());
                }
            }
            Entry {
                title: title.into(),
                keywords: sub.clone(),
                subtitle: Some(sub.join(" · ").into()),
                keys: None,
                target: Target::Mr(m.key.clone()),
                icon: Some(("gitlab", crate::links::Kind::Mr.color())),
                current: false,
            }
        })
        .collect()
}

impl AbroMux {
    /// ⌘⇧P / ⌘P: open the palette in `mode`, switch an open one to it, or
    /// close it if it's already showing `mode`.
    pub(crate) fn toggle_palette(
        &mut self,
        mode: Mode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let prev_focus = match self.palette.take() {
            Some(p) if p.mode == mode => {
                self.restore_focus(p.prev_focus, window, cx);
                return cx.notify();
            }
            // Switching modes: focus goes back first, so commands are
            // worked out for the pane rather than the palette's input.
            Some(p) => {
                if let Some(f) = &p.prev_focus {
                    window.focus(f);
                }
                p.prev_focus
            }
            None => window.focused(cx),
        };
        self.notifications_open = false;
        let entries = match mode {
            Mode::Commands => self.command_entries(window, cx),
            Mode::Workspaces => self.workspace_entries(cx),
            Mode::OpenLink(_) | Mode::Prompt => vec![],
            Mode::MergeRequests => mr_entries(cx),
        };
        let placeholder = match mode {
            Mode::Commands => "Type a command".to_string(),
            Mode::Workspaces => "Search workspaces and tabs".to_string(),
            Mode::OpenLink(_) => "Paste a link".to_string(),
            Mode::MergeRequests => "Search merge requests, or paste a link".to_string(),
            Mode::Prompt => String::new(),
        };
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        // A link of that kind on the clipboard is there to start with,
        // selected: Enter opens it, a paste replaces it.
        let clipboard_link = match mode {
            Mode::OpenLink(id) => cx
                .read_from_clipboard()
                .and_then(|c| c.text())
                .map(|t| t.trim().to_string())
                .filter(|t| crate::plugins::find(id).is_some_and(|p| (p.claims)(t))),
            _ => None,
        };
        if let Some(link) = &clipboard_link {
            input.update(cx, |i, cx| i.set_value(link.clone(), window, cx));
        }
        cx.subscribe_in(
            &input,
            window,
            |this, input, ev: &InputEvent, _, cx| match ev {
                InputEvent::Change
                    if this
                        .palette
                        .as_ref()
                        .is_some_and(|p| matches!(p.mode, Mode::OpenLink(_))) =>
                {
                    // The line under the input follows what's typed.
                    cx.notify()
                }
                InputEvent::Change => {
                    let query = input.read(cx).value().to_string();
                    if let Some(p) = &mut this.palette {
                        p.filter(&query);
                        cx.notify();
                    }
                }
                // Focus went somewhere else (a click outside, a pane taking it).
                InputEvent::Blur if this.palette.as_ref().is_some_and(|p| &p.input == input) => {
                    this.palette = None;
                    cx.notify();
                }
                _ => {}
            },
        )
        .detach();
        if clipboard_link.is_some() {
            crate::focus_selected(&input, window, cx);
        } else {
            input.update(cx, |i, cx| i.focus(window, cx));
        }
        let tabs = (mode == Mode::Workspaces).then(|| Column {
            entries: self.tab_entries(self.active, cx),
            matches: vec![],
            selected: 0,
            scroll: ScrollHandle::new(),
        });
        let mut palette = Palette {
            mode,
            input,
            entries,
            matches: vec![],
            selected: 0,
            prev_focus,
            scroll: ScrollHandle::new(),
            tabs,
            on_tabs: false,
            prompt: None,
            query: String::new(),
        };
        palette.filter("");
        self.palette = Some(palette);
        cx.notify();
    }

    /// Show a script's question in the palette, in place of whatever it
    /// showed. Returns the prompt's id, for `close_prompt`.
    pub(crate) fn open_prompt(
        &mut self,
        mut prompt: Prompt,
        items: Vec<PromptItem>,
        placeholder: Option<String>,
        value: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        prompt.id = NEXT.fetch_add(1, Ordering::Relaxed);
        let id = prompt.id;
        if self
            .palette
            .as_ref()
            .is_some_and(|p| p.mode == Mode::Prompt)
        {
            // Toggling would only close it.
            self.palette = None;
        }
        self.toggle_palette(Mode::Prompt, window, cx);
        let Some(p) = &mut self.palette else {
            return id;
        };
        p.entries = items
            .into_iter()
            .map(|(id, title, subtitle, detail)| Entry {
                title: title.into(),
                subtitle: subtitle.map(Into::into),
                keywords: vec![id.clone()],
                keys: detail.map(Into::into),
                target: Target::Choice(id),
                icon: None,
                current: false,
            })
            .collect();
        let input = p.input.clone();
        // The title shows above the field, so the hint doesn't repeat it.
        let hint = placeholder.unwrap_or_else(|| {
            if prompt.pick {
                "Type to filter".into()
            } else {
                String::new()
            }
        });
        input.update(cx, |i, cx| {
            i.set_placeholder(hint, window, cx);
            if let Some(v) = &value {
                i.set_value(v.clone(), window, cx);
            }
        });
        p.prompt = Some(prompt);
        let typed = input.read(cx).value().to_string();
        p.filter(&typed);
        cx.notify();
        id
    }

    /// Close prompt `id` if it's still showing (its time ran out, or the
    /// script went away); the asker hears it was cancelled.
    pub(crate) fn close_prompt(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .palette
            .as_ref()
            .and_then(|p| p.prompt.as_ref())
            .is_some_and(|q| q.id == id)
        {
            self.close_palette(window, cx);
        }
    }

    pub(crate) fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = self.palette.take() {
            self.restore_focus(p.prev_focus, window, cx);
            cx.notify();
        }
    }

    fn restore_focus(
        &mut self,
        prev: Option<FocusHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match prev {
            Some(f) => window.focus(&f),
            None => self.focus_active(window, cx),
        }
    }

    fn confirm_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut p) = self.palette.take() else {
            return;
        };
        if p.mode == Mode::Prompt {
            let typed = p.input.read(cx).value().to_string();
            // Typed in the same moment as Enter: its filtering hasn't run yet.
            if p.query != typed {
                p.filter(&typed);
            }
            let chosen = p
                .matches
                .get(p.selected)
                .map(|&(ix, _)| &p.entries[ix].target);
            let answer = match (&p.prompt, chosen) {
                (Some(q), _) if !q.pick => Some(serde_json::json!(typed)),
                (Some(_), Some(Target::Choice(id))) => Some(serde_json::json!(id)),
                (Some(q), None) if q.allow_other && !typed.trim().is_empty() => {
                    Some(serde_json::json!({ "text": typed }))
                }
                _ => None,
            };
            let Some(answer) = answer else {
                // Nothing to answer with yet: it stays.
                self.palette = Some(p);
                return;
            };
            if let Some(tx) = p.prompt.as_mut().and_then(|q| q.answer.take()) {
                let _ = tx.send(serde_json::json!({ "answer": answer }));
            }
            self.restore_focus(p.prev_focus, window, cx);
            return cx.notify();
        }
        if let Mode::OpenLink(id) = p.mode {
            let url = p.input.read(cx).value().trim().to_string();
            // Not one it opens: the palette stays, saying so.
            if !crate::plugins::find(id).is_some_and(|pl| (pl.claims)(&url)) {
                self.palette = Some(p);
                return cx.notify();
            }
            self.restore_focus(p.prev_focus, window, cx);
            return self.open_plugin_link(id, &url, window, cx);
        }
        // ⌘P with the tabs column selected: its row is the one chosen.
        if p.on_tabs
            && let Some(mut col) = p.tabs.take()
        {
            match col.matches.get(col.selected) {
                Some(&(ix, _)) => {
                    p.entries = vec![col.entries.swap_remove(ix)];
                    p.matches = vec![(0, vec![])];
                    p.selected = 0;
                }
                None => {
                    p.tabs = Some(col);
                    self.palette = Some(p);
                    return;
                }
            }
        }
        let Some(&(ix, _)) = p.matches.get(p.selected) else {
            let typed = p.input.read(cx).value().to_string();
            if p.mode == Mode::MergeRequests && crate::gitlab::model::MrKey::parse(&typed).is_some()
            {
                cx.notify();
                return {
                    if let Err(e) = self.open_mr_ref(&typed, window, cx) {
                        gpui_component::WindowExt::push_notification(
                            window,
                            gpui_component::notification::Notification::error(format!("{e:#}")),
                            cx,
                        );
                    }
                };
            }
            self.palette = Some(p);
            return;
        };
        let entry = p.entries.swap_remove(ix);
        cx.notify();
        match entry.target {
            Target::Action(action) => {
                // Dispatched where focus was, so terminal and browser
                // commands reach their view.
                self.restore_focus(p.prev_focus, window, cx);
                window.dispatch_action(action, cx);
            }
            Target::Workspace(id) => match self.workspaces.iter().position(|w| w.id == id) {
                Some(i) => self.select_workspace(i, window, cx),
                None => self.restore_focus(p.prev_focus, window, cx),
            },
            Target::Mr(k) => self.open_mr(k, window, cx),
            // Answered above.
            Target::Choice(_) => {}
            Target::Tab(id) => {
                if self.pin_workspace(id).is_some() {
                    self.reveal_tab(id, window, cx);
                } else {
                    self.restore_focus(p.prev_focus, window, cx);
                }
            }
            Target::Link(ws, link) => match self.ws_index(ws) {
                Some(wi) => {
                    if wi != self.active {
                        self.select_workspace(wi, window, cx);
                    }
                    self.show_link(wi, &link, true, false, window, cx);
                }
                None => self.restore_focus(p.prev_focus, window, cx),
            },
        }
    }

    /// Workspace `wi`'s tabs and suggestions as ⌘P rows.
    fn tab_entries(&self, wi: usize, cx: &App) -> Vec<Entry> {
        let ws = &self.workspaces[wi];
        let focused = ws.focused_surface().map(|s| s.entity_id());
        let items = self.items(wi, cx);
        let detail = |s: &Surface| -> Option<String> {
            match s {
                Surface::Browser(b) => Some(b.read(cx).url.clone()),
                Surface::Terminal(_) => s.cwd(cx).map(|c| tilde(&c)),
                Surface::Plugin(_) => None,
            }
        };
        let subtitle = |parts: Vec<String>| -> Option<SharedString> {
            let parts: Vec<String> = parts.into_iter().filter(|p| !p.is_empty()).collect();
            (!parts.is_empty()).then(|| parts.join("  ·  ").into())
        };
        let title_of = |s: &Surface, link: Option<&crate::links::Link>| -> String {
            match link {
                Some(l) if l.kind != crate::links::Kind::Web => match l.label() {
                    Some(t) => format!("{} {t}", l.short_key()),
                    None => l.short_key(),
                },
                _ => s.title(cx),
            }
        };
        // As the sidebar draws it: a ticket's or merge request's mark, else
        // the tab's own (an agent's logo, a terminal, a page).
        let icon_of = |s: &Surface, link: Option<&crate::links::Link>| match link {
            Some(l) if l.kind != crate::links::Kind::Web => Some((l.kind.icon(), l.kind.color())),
            _ => Some((s.icon(cx), None)),
        };
        let mut out = vec![];
        let mut listed = vec![];
        for (i, it) in items.iter().enumerate() {
            let keys = (i < crate::links::DIGITS)
                .then(|| crate::keymap::default_label(&format!("link{}", i + 1)).into());
            match (&it.tab, &it.link) {
                (Some((s, hidden)), link) => {
                    listed.push(s.entity_id());
                    let state = if *hidden { "pinned, hidden" } else { "pinned" };
                    out.push(Entry {
                        title: title_of(s, link.as_ref()).into(),
                        subtitle: subtitle(vec![state.into(), detail(s).unwrap_or_default()]),
                        keywords: link.iter().map(|l| l.key.clone()).collect(),
                        keys,
                        target: Target::Tab(s.entity_id()),
                        icon: icon_of(s, link.as_ref()),
                        current: Some(s.entity_id()) == focused,
                    });
                }
                (None, Some(l)) => out.push(Entry {
                    title: title_of_link(l).into(),
                    subtitle: subtitle(vec!["suggested".into(), l.url.clone()]),
                    keywords: vec![l.key.clone()],
                    keys,
                    target: Target::Link(ws.id, l.clone()),
                    icon: Some((l.kind.icon(), l.kind.color())),
                    current: false,
                }),
                (None, None) => {}
            }
        }
        // Pinned ones come first; the rest follow in layout order.
        let rest: Vec<Entry> = ws
            .surfaces()
            .filter(|s| !listed.contains(&s.entity_id()))
            .map(|s| {
                let link = self.tab_link(wi, s, cx);
                Entry {
                    title: title_of(s, link.as_ref()).into(),
                    subtitle: subtitle(vec![detail(s).unwrap_or_default()]),
                    keywords: link.iter().map(|l| l.key.clone()).collect(),
                    keys: None,
                    target: Target::Tab(s.entity_id()),
                    icon: icon_of(s, link.as_ref()),
                    current: Some(s.entity_id()) == focused,
                }
            })
            .collect();
        // Suggestions go last, after the open tabs.
        let split = out
            .iter()
            .position(|e| matches!(e.target, Target::Link(..)))
            .unwrap_or(out.len());
        let suggestions = out.split_off(split);
        out.extend(rest);
        out.extend(suggestions);
        out
    }

    /// The commands that apply to what has focus now, with their shortcuts.
    fn command_entries(&self, window: &Window, cx: &mut App) -> Vec<Entry> {
        let browser = matches!(self.ws().focused_surface(), Some(Surface::Browser(_)));
        let project = self
            .focused_cwd(cx)
            .and_then(|d| settings::find_project(Path::new(&d)));
        let available: Vec<_> = commands(project.as_deref(), cx)
            .into_iter()
            // Root actions are always available; this drops terminal ones
            // when a browser has focus and the other way round.
            .filter(|(.., a)| window.is_action_available(a.as_ref(), cx))
            // Its handler sits on the root, but it only acts on a browser.
            .filter(|(.., a)| browser || !a.as_any().is::<crate::FocusAddressBar>())
            .collect();
        let stack = window.context_stack();
        let keymap = cx.key_bindings();
        let keymap = keymap.borrow();
        available
            .into_iter()
            .map(|(title, subtitle, keywords, action)| {
                let title = match title.as_str() {
                    "Quit" => format!("Quit {}", channel::app_name()),
                    "About" => format!("About {}", channel::app_name()),
                    _ => title,
                };
                // The first binding whose context applies is the main one
                // (⌘⇧] before ⌃⇥). gpui's own lookup would hide ⌥⌘I behind
                // gpui-component's context-free inspector binding, which
                // never runs.
                let keys = keymap
                    .bindings_for_action(action.as_ref())
                    .find(|b| b.predicate().is_none_or(|p| p.depth_of(&stack).is_some()))
                    .map(|b| {
                        b.keystrokes()
                            .iter()
                            .map(|k| key_label(k.modifiers(), k.key()))
                            .collect::<Vec<_>>()
                            .join(" ")
                            .into()
                    });
                Entry {
                    title: title.into(),
                    subtitle: subtitle.map(Into::into),
                    keywords,
                    keys,
                    target: Target::Action(action),
                    icon: None,
                    current: false,
                }
            })
            .collect()
    }

    fn workspace_entries(&self, cx: &App) -> Vec<Entry> {
        let n = self.workspaces.len();
        self.workspaces
            .iter()
            .enumerate()
            .map(|(i, ws)| {
                // A workspace on another machine says which, and is found
                // by its server's name too.
                let server = ws.host.as_deref().map(crate::servers::label);
                let cwd = ws.cwd(cx).map(|c| tilde(&c));
                let subtitle = match (&server, cwd) {
                    (Some(s), Some(c)) => Some(format!("on {s} · {c}")),
                    (Some(s), None) => Some(format!("on {s}")),
                    (None, c) => c,
                };
                Entry {
                    keywords: server.into_iter().chain(ws.host.clone()).collect(),
                    title: ws.name(cx).into(),
                    subtitle: subtitle.map(Into::into),
                    keys: shortcut_digit(i, n)
                        .map(|d| crate::keymap::default_label(&format!("workspace{d}")).into()),
                    target: Target::Workspace(ws.id),
                    icon: Some((if ws.host.is_some() { "globe" } else { "folder" }, None)),
                    current: i == self.active,
                }
            })
            .collect()
    }

    /// Handlers for the palette's keys, put on the root element.
    pub(crate) fn palette_actions(
        &self,
        root: Stateful<Div>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        root.on_action(cx.listener(|this, _: &SelectNext, _, cx| this.move_selection(1, cx)))
            .on_action(cx.listener(|this, _: &SelectPrev, _, cx| this.move_selection(-1, cx)))
            .on_action(
                cx.listener(|this, _: &Confirm, window, cx| this.confirm_palette(window, cx)),
            )
            .on_action(cx.listener(|this, _: &Dismiss, window, cx| this.close_palette(window, cx)))
            .on_action(cx.listener(|this, _: &SwitchColumn, _, cx| this.switch_column(cx)))
    }

    fn move_selection(&mut self, by: isize, cx: &mut Context<Self>) {
        let Some(p) = &mut self.palette else { return };
        let (matches, selected, scroll) = match (&mut p.tabs, p.on_tabs) {
            (Some(c), true) => (&c.matches, &mut c.selected, &c.scroll),
            _ => (&p.matches, &mut p.selected, &p.scroll),
        };
        let n = matches.len() as isize;
        if n == 0 {
            return;
        }
        *selected = (*selected as isize + by).rem_euclid(n) as usize;
        scroll.scroll_to_item(*selected);
        cx.notify();
    }

    /// Tab in ⌘P: the other column takes the selection.
    fn switch_column(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = &mut self.palette
            && p.tabs.is_some()
        {
            p.on_tabs = !p.on_tabs;
            cx.notify();
        }
    }

    /// One column's rows. `active`: this column has the selection, so its
    /// selected row is lit (the other's only faintly).
    fn palette_rows(
        entries: &[Entry],
        matches: &[(usize, Vec<usize>)],
        selected: usize,
        active: bool,
        tabs: bool,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        matches
            .iter()
            .enumerate()
            .map(|(row, (ix, positions))| {
                let e = &entries[*ix];
                let lit = row == selected && active;
                let id: ElementId = if tabs {
                    ("palette-tab-row", row).into()
                } else {
                    ("palette-row", row).into()
                };
                div()
                    .id(id)
                    .mx(px(6.))
                    .px(px(10.))
                    .py(px(6.))
                    .rounded(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .cursor_pointer()
                    .when(lit, |d| d.bg(tint(0x1a)))
                    .when(row == selected && !active, |d| d.bg(tint(0x0a)))
                    .when(!lit, |d| d.hover(|s| s.bg(rgba(pal().hover))))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(p) = &mut this.palette {
                            p.on_tabs = tabs;
                            match (&mut p.tabs, tabs) {
                                (Some(c), true) => c.selected = row,
                                _ => p.selected = row,
                            }
                        }
                        this.confirm_palette(window, cx);
                    }))
                    .children(e.icon.map(|(name, color)| {
                        icon(name)
                            .size(px(14.))
                            .text_color(color.map_or(rgb(pal().muted), rgb))
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_baseline()
                            .gap(px(8.))
                            .child(
                                div()
                                    .flex_none()
                                    .max_w(px(260.))
                                    .truncate()
                                    .text_size(px(13.))
                                    .when(e.current, |d| d.font_weight(FontWeight::SEMIBOLD))
                                    .child(highlighted(&e.title, positions)),
                            )
                            .children(e.subtitle.clone().map(|s| {
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(11.5))
                                    .text_color(rgb(pal().muted))
                                    .child(s)
                            })),
                    )
                    .when(e.current, |d| {
                        d.child(
                            div()
                                .flex_none()
                                .size(px(6.))
                                .rounded_full()
                                .bg(rgb(pal().accent)),
                        )
                    })
                    .children(e.keys.clone().map(|k| {
                        div()
                            .flex_none()
                            .text_size(px(11.5))
                            .text_color(rgb(pal().muted))
                            .child(k)
                    }))
                    .into_any_element()
            })
            .collect()
    }

    /// A scrolling list of rows, with a heading when it's one of ⌘P's
    /// columns (lit when it has the selection).
    fn palette_column(
        id: &'static str,
        heading: Option<(&'static str, bool)>,
        scroll: &ScrollHandle,
        rows: Vec<AnyElement>,
        empty: SharedString,
    ) -> Div {
        div()
            .flex()
            .flex_col()
            .children(heading.map(|(h, lit)| {
                div()
                    .px(px(16.))
                    .pt(px(8.))
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(if lit {
                        rgb(pal().text)
                    } else {
                        rgb(pal().muted)
                    })
                    .child(h)
            }))
            .child(
                div()
                    .id(id)
                    .max_h(px(MAX_ROWS_HEIGHT))
                    .py(px(6.))
                    .overflow_y_scroll()
                    .track_scroll(scroll)
                    .when(rows.is_empty(), |d| {
                        d.child(
                            div()
                                .px(px(16.))
                                .py(px(8.))
                                .text_size(px(12.5))
                                .text_color(rgb(pal().muted))
                                .child(empty),
                        )
                    })
                    .children(rows),
            )
    }

    pub(crate) fn render_palette(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = self.palette.as_ref()?;
        let rows = Self::palette_rows(&p.entries, &p.matches, p.selected, !p.on_tabs, false, cx);
        let tab_rows = p
            .tabs
            .as_ref()
            .map(|c| Self::palette_rows(&c.entries, &c.matches, c.selected, p.on_tabs, true, cx));
        let empty: SharedString = match p.mode {
            Mode::Commands => "No commands match your search.".into(),
            Mode::Workspaces => "No workspaces match your search.".into(),
            Mode::Prompt => {
                let typed = p.input.read(cx).value().to_string();
                let q = p.prompt.as_ref();
                let confirm = q.and_then(|q| q.confirm.clone());
                match q {
                    Some(q) if !q.pick => {
                        let what = confirm.unwrap_or_else(|| format!("Use “{}”", typed.trim()));
                        format!("↵ {what}  ·  esc cancel").into()
                    }
                    Some(q) if q.allow_other && !typed.trim().is_empty() => {
                        format!("↵ Use “{}”  ·  esc cancel", typed.trim()).into()
                    }
                    _ => "Nothing matches.  ·  esc cancel".into(),
                }
            }
            Mode::MergeRequests => {
                if crate::gitlab::model::MrKey::parse(&p.input.read(cx).value()).is_some() {
                    "Enter to open this merge request".into()
                } else {
                    "No cached merge requests match. Paste a link to open any.".into()
                }
            }
            Mode::OpenLink(id) => {
                let url = p.input.read(cx).value().trim().to_string();
                let plugin = crate::plugins::find(id);
                let what = plugin
                    .and_then(|pl| pl.open_link)
                    .unwrap_or("Open link")
                    .trim_end_matches('…');
                match plugin {
                    _ if url.is_empty() => {
                        format!("{what}: paste its link  ·  Esc to cancel").into()
                    }
                    Some(pl) if (pl.claims)(&url) => {
                        "Enter to open it in a new tab  ·  Esc to cancel".into()
                    }
                    _ => format!("Not a link this opens ({what})  ·  Esc to cancel").into(),
                }
            }
        };
        Some(
            div()
                .absolute()
                .inset_0()
                // Clicking outside closes it.
                .child(
                    div()
                        .id("palette-backdrop")
                        .absolute()
                        .inset_0()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| this.close_palette(window, cx)),
                        ),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(TITLEBAR_H + 8.))
                        .left_0()
                        .right_0()
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .id("palette")
                                .key_context(CONTEXT)
                                .w(px(if p.tabs.is_some() { SWITCHER_WIDTH } else { WIDTH }))
                                .max_w(relative(0.94))
                                .flex()
                                .flex_col()
                                .rounded(px(10.))
                                .bg(rgb(pal().tabbar_bg))
                                .border_1()
                                .border_color(rgb(pal().border))
                                .shadow_lg()
                                .overflow_hidden()
                                .occlude()
                                // Keep focus in the input when a row is clicked.
                                .on_mouse_down(MouseButton::Left, |_, window, _| {
                                    window.prevent_default()
                                })
                                .when_some(p.prompt.as_ref().map(|q| q.title.clone()), |d, t| {
                                    d.child(
                                        div()
                                            .px(px(14.))
                                            .pt(px(9.))
                                            .text_size(px(11.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(pal().muted))
                                            .child(t),
                                    )
                                })
                                .child(
                                    div()
                                        .h(px(40.))
                                        .px(px(14.))
                                        .flex()
                                        .items_center()
                                        .border_b_1()
                                        .border_color(rgb(pal().border))
                                        .child(
                                            Input::new(&p.input)
                                                .appearance(false)
                                                .text_size(px(14.))
                                                .h_full()
                                                .px_0()
                                                .py_0(),
                                        ),
                                )
                                .child(match tab_rows {
                                    None => Self::palette_column(
                                        "palette-rows",
                                        None,
                                        &p.scroll,
                                        rows,
                                        empty,
                                    )
                                    .into_any_element(),
                                    Some(tab_rows) => div()
                                        .flex()
                                        .child(
                                            Self::palette_column(
                                                "palette-rows",
                                                Some(("Workspaces", !p.on_tabs)),
                                                &p.scroll,
                                                rows,
                                                empty,
                                            )
                                            .flex_1()
                                            .min_w_0()
                                            .border_r_1()
                                            .border_color(rgb(pal().border)),
                                        )
                                        .children(p.tabs.as_ref().map(|c| {
                                            Self::palette_column(
                                                "palette-tab-rows",
                                                Some(("Tabs in this workspace", p.on_tabs)),
                                                &c.scroll,
                                                tab_rows,
                                                "No tabs here match your search.".into(),
                                            )
                                            .flex_1()
                                            .min_w_0()
                                        }))
                                        .into_any_element(),
                                })
                                .when_some(
                                    p.prompt
                                        .as_ref()
                                        .filter(|q| q.pick && !p.matches.is_empty())
                                        .map(|q| q.confirm.clone().unwrap_or("choose".into())),
                                    |d, confirm| {
                                        d.child(
                                            div()
                                                .px(px(14.))
                                                .py(px(6.))
                                                .border_t_1()
                                                .border_color(rgb(pal().border))
                                                .text_size(px(11.))
                                                .text_color(rgb(pal().muted))
                                                .child(format!(
                                                    "↑↓ select  ·  ↵ {confirm}  ·  esc cancel"
                                                )),
                                        )
                                    },
                                )
                                .when(p.tabs.is_some(), |d| {
                                    d.child(
                                        div()
                                            .px(px(14.))
                                            .py(px(6.))
                                            .border_t_1()
                                            .border_color(rgb(pal().border))
                                            .text_size(px(11.))
                                            .text_color(rgb(pal().muted))
                                            .child(
                                                "⇥ switch column  ·  ↑↓ select  ·  ↵ go  ·  esc close",
                                            ),
                                    )
                                }),
                        ),
                )
                .into_any_element(),
        )
    }
}

impl Palette {
    fn filter(&mut self, query: &str) {
        self.query = query.to_string();
        self.matches = rank(&self.entries, query);
        self.selected = 0;
        self.scroll.scroll_to_item(0);
        if let Some(c) = &mut self.tabs {
            c.matches = rank(&c.entries, query);
            c.selected = 0;
            c.scroll.scroll_to_item(0);
        }
        // Typing that leaves only tabs: go to the tabs.
        if self.matches.is_empty() && self.tabs.as_ref().is_some_and(|c| !c.matches.is_empty()) {
            self.on_tabs = true;
        }
    }
}

/// `entries` that match `query`, best first, with the matched character
/// positions of each title.
fn rank(entries: &[Entry], query: &str) -> Vec<(usize, Vec<usize>)> {
    {
        let mut scored: Vec<(i32, usize, Vec<usize>)> = entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| match fuzzy_match(query, &e.title) {
                Some((score, pos)) => Some((score, i, pos)),
                // A keyword match ranks below any title match.
                None => e
                    .keywords
                    .iter()
                    .filter_map(|k| fuzzy_match(query, k))
                    .map(|(score, _)| (score - 2000, i, vec![]))
                    .max_by_key(|(score, ..)| *score),
            })
            .collect();
        // Best score first; ties keep palette order.
        scored.sort_by_key(|(score, i, _)| (-score, *i));
        scored.into_iter().map(|(_, i, pos)| (i, pos)).collect()
    }
}

/// A suggested link's row title: its key and title.
fn title_of_link(l: &crate::links::Link) -> String {
    match l.label() {
        Some(t) => format!("{} {t}", l.short_key()),
        None => l.short_key(),
    }
}

/// The title with matched characters in the accent colour.
fn highlighted(title: &SharedString, positions: &[usize]) -> StyledText {
    let style = HighlightStyle {
        color: Some(rgb(pal().accent_bright).into()),
        font_weight: Some(FontWeight::SEMIBOLD),
        ..Default::default()
    };
    let highlights = title
        .char_indices()
        .enumerate()
        .filter(|(ci, _)| positions.contains(ci))
        .map(|(_, (b, c))| (b..b + c.len_utf8(), style))
        .collect::<Vec<_>>();
    StyledText::new(title.clone()).with_highlights(highlights)
}

/// A shortcut as this OS shows it: `⇧⌘P` on macOS, `Ctrl+Shift+P` on Linux.
pub(crate) fn key_label(m: &Modifiers, key: &str) -> String {
    if abromux_platform::MACOS {
        mac_key_label(m, key)
    } else {
        linux_key_label(m, key)
    }
}

/// gpui folds shift into these keys (cmd-} is ⌘⇧]): whether shift is held,
/// and the key it was on.
fn unfold_shift<'a>(m: &Modifiers, key: &'a str) -> (bool, &'a str) {
    match key {
        "{" => (true, "["),
        "}" => (true, "]"),
        "<" => (true, ","),
        k => (m.shift, k),
    }
}

/// The way Linux apps write shortcuts: `Ctrl+Alt+Shift+Super+Key`.
fn linux_key_label(m: &Modifiers, key: &str) -> String {
    let (shift, key) = unfold_shift(m, key);
    let mut parts: Vec<String> = [
        (m.control, "Ctrl"),
        (m.alt, "Alt"),
        (shift, "Shift"),
        (m.platform, "Super"),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .map(|(_, name)| name.to_string())
    .collect();
    parts.push(match key {
        "pageup" => "PageUp".into(),
        "pagedown" => "PageDown".into(),
        "escape" => "Esc".into(),
        "+" => "Plus".into(),
        k if k.chars().count() == 1 => k.to_uppercase(),
        k => {
            let mut c = k.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect())
                .unwrap_or_default()
        }
    });
    parts.join("+")
}

/// A shortcut the way macOS menus show it: ⌃⌥⇧⌘ then the key.
fn mac_key_label(m: &Modifiers, key: &str) -> String {
    let (shift, key) = unfold_shift(m, key);
    let mut s = String::new();
    for (on, sym) in [
        (m.control, '⌃'),
        (m.alt, '⌥'),
        (shift, '⇧'),
        (m.platform, '⌘'),
    ] {
        if on {
            s.push(sym);
        }
    }
    s.push_str(&match key {
        "left" => "←".into(),
        "right" => "→".into(),
        "up" => "↑".into(),
        "down" => "↓".into(),
        "tab" => "⇥".into(),
        "enter" => "↩".into(),
        "escape" => "⎋".into(),
        "backspace" => "⌫".into(),
        "space" => "Space".into(),
        k => k.to_uppercase(),
    });
    s
}

/// Case-insensitive fuzzy match of `query` in `text`. Returns a score
/// (higher is better) and the char positions matched, or None. A substring
/// beats scattered letters; matches at word starts and the start of the
/// text score higher.
pub fn fuzzy_match(query: &str, text: &str) -> Option<(i32, Vec<usize>)> {
    let q: Vec<char> = query.trim().chars().flat_map(char::to_lowercase).collect();
    if q.is_empty() {
        return Some((0, vec![]));
    }
    let t: Vec<char> = text
        .chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect();
    let word_start = |i: usize| i == 0 || !t[i - 1].is_alphanumeric();

    // A contiguous run, preferring one that starts a word.
    let runs = (0..t.len()).filter(|&i| t[i..].starts_with(&q));
    if let Some(start) = runs
        .clone()
        .find(|&i| word_start(i))
        .or_else(|| runs.clone().next())
    {
        let score =
            1000 + if start == 0 {
                200
            } else if word_start(start) {
                100
            } else {
                0
            } - start as i32;
        return Some((score, (start..start + q.len()).collect()));
    }

    // Letters in order, each taken at a word start if one is ahead before
    // the next letter is needed, else the first occurrence.
    let mut pos = Vec::with_capacity(q.len());
    let mut from = 0;
    for (qi, &c) in q.iter().enumerate() {
        let rest = &q[qi + 1..];
        let candidates: Vec<usize> = (from..t.len()).filter(|&i| t[i] == c).collect();
        let fits = |i: usize| is_subsequence(rest, &t[i + 1..]);
        let pick = candidates
            .iter()
            .copied()
            .find(|&i| word_start(i) && fits(i))
            .or_else(|| candidates.iter().copied().find(|&i| fits(i)))?;
        pos.push(pick);
        from = pick + 1;
    }
    let starts = pos.iter().filter(|&&i| word_start(i)).count() as i32;
    let gaps: i32 = pos.windows(2).map(|w| (w[1] - w[0] - 1) as i32).sum();
    Some((500 + 20 * starts - gaps - pos[0] as i32, pos))
}

fn is_subsequence(needle: &[char], hay: &[char]) -> bool {
    let mut it = hay.iter();
    needle.iter().all(|c| it.any(|h| h == c))
}

#[cfg(test)]
mod tests {
    use super::{fuzzy_match, linux_key_label, mac_key_label};
    use gpui::Modifiers;

    fn score(q: &str, t: &str) -> i32 {
        fuzzy_match(q, t).map(|(s, _)| s).unwrap_or(i32::MIN)
    }

    #[test]
    fn matches_and_positions() {
        assert_eq!(fuzzy_match("", "Split Right"), Some((0, vec![])));
        assert_eq!(
            fuzzy_match("right", "Split Right").unwrap().1,
            vec![6, 7, 8, 9, 10]
        );
        assert_eq!(fuzzy_match("SR", "Split Right").unwrap().1, vec![0, 6]);
        assert_eq!(
            fuzzy_match("ntt", "New Terminal Tab").unwrap().1,
            vec![0, 4, 13]
        );
        assert!(fuzzy_match("xyz", "Split Right").is_none());
        assert!(fuzzy_match("thgir", "Split Right").is_none());
    }

    #[test]
    fn ranking() {
        // A substring beats letters spread out.
        assert!(score("tab", "Next Tab") > score("tab", "Toggle Address Bar"));
        // A word start beats the middle of a word.
        assert!(score("in", "Increase Font Size") > score("in", "Find Next"));
        // Earlier is better.
        assert!(score("split", "Split Down") > score("split", "Something Split"));
        // Word starts beat scattered letters.
        assert!(score("cw", "Close Workspace") > score("cw", "Clear Scrollback Window"));
    }

    #[test]
    fn non_ascii() {
        assert_eq!(fuzzy_match("ž", "Žilvinas").unwrap().1, vec![0]);
        assert!(fuzzy_match("rename", "Rename Workspace…").is_some());
    }

    #[test]
    fn key_labels() {
        let cmd = Modifiers {
            platform: true,
            ..Default::default()
        };
        let cmd_shift = Modifiers {
            platform: true,
            shift: true,
            ..Default::default()
        };
        let ctrl_shift = Modifiers {
            control: true,
            shift: true,
            ..Default::default()
        };
        assert_eq!(mac_key_label(&cmd, "}"), "⇧⌘]");
        assert_eq!(mac_key_label(&cmd, "<"), "⇧⌘,");
        assert_eq!(mac_key_label(&cmd_shift, "p"), "⇧⌘P");
        assert_eq!(mac_key_label(&ctrl_shift, "h"), "⌃⇧H");
        assert_eq!(
            mac_key_label(
                &Modifiers {
                    alt: true,
                    platform: true,
                    ..Default::default()
                },
                "left"
            ),
            "⌥⌘←"
        );
        assert_eq!(linux_key_label(&ctrl_shift, "p"), "Ctrl+Shift+P");
        assert_eq!(linux_key_label(&ctrl_shift, "<"), "Ctrl+Shift+,");
        assert_eq!(linux_key_label(&ctrl_shift, "tab"), "Ctrl+Shift+Tab");
        let ctrl_alt = Modifiers {
            control: true,
            alt: true,
            ..Default::default()
        };
        assert_eq!(linux_key_label(&ctrl_alt, "pagedown"), "Ctrl+Alt+PageDown");
        assert_eq!(linux_key_label(&ctrl_alt, "left"), "Ctrl+Alt+Left");
        assert_eq!(linux_key_label(&cmd, "+"), "Super+Plus");
    }
}
