//! The interface between abromux and its plugins.
//!
//! A plugin is a crate built into abromux that adds a kind of tab. It
//! implements [`Tab`] for its view and hands abromux a [`Plugin`]: abromux
//! then shows those tabs in panes like terminals and browsers, saves them
//! with the session ([`Tab::save`]) and reopens them from that
//! ([`Plugin::open`]), and reports them over the control socket
//! ([`Tab::state`]). The plugin never sees abromux's own types.

use gpui::{AnyView, App, Entity, EntityId, FocusHandle, Focusable, Render, Window};
use serde_json::Value;
use std::{cell::RefCell, rc::Rc};

/// The view of a plugin tab.
pub trait Tab: Render + Focusable {
    /// The tab bar's title.
    fn title(&self, cx: &App) -> String;

    /// The tab's icon, by name: one of abromux's `assets/icons`.
    fn icon(&self) -> &'static str;

    /// What reopens it after a restart; [`Plugin::open`] gets it back.
    fn save(&self, cx: &App) -> Value;

    /// What it's showing, as JSON, for `abromux --json tree` and tests.
    fn state(&self, cx: &App) -> Value {
        self.save(cx)
    }

    /// Do `name` with `args`, for `abromux rpc surface.plugin` (scripts and
    /// tests drive the tab with it). The answer goes back to the caller.
    fn command(
        &mut self,
        name: &str,
        args: &Value,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> anyhow::Result<Value> {
        let _ = (args, window, cx);
        anyhow::bail!("this tab has no command {name:?}")
    }
}

/// What abromux does for plugins, set as a gpui global at startup.
pub struct Host {
    /// Open a web page in abromux's browser, beside the focused tab.
    pub open_url: fn(&str, &mut Window, &mut App),
    /// Run something in a terminal, see [`Run`].
    pub run_terminal: fn(Run, &mut Window, &mut App),
}

/// A command for a terminal: started in a new one beside the tab `beside`
/// (in the pane next to its own, else a new split), or typed into the
/// terminal `send_to` (a ref like `surface:3`).
#[derive(Clone, Debug, Default)]
pub struct Run {
    pub beside: Option<EntityId>,
    pub cwd: Option<String>,
    pub command: Option<String>,
    pub send_to: Option<String>,
    /// For `send_to`: the text, entered.
    pub text: Option<String>,
    /// The new terminal's tab name.
    pub title: Option<String>,
}

/// Run `run` in a terminal of abromux's. Without abromux (tests) it fails.
pub fn run_terminal(run: Run, window: &mut Window, cx: &mut App) -> Result<(), String> {
    match cx.try_global::<Host>().map(|h| h.run_terminal) {
        Some(f) => {
            f(run, window, cx);
            Ok(())
        }
        None => Err("no abromux to run it in".into()),
    }
}

impl gpui::Global for Host {}

/// Open `url` in abromux's browser, or the system's without abromux (tests).
pub fn open_url(url: &str, window: &mut Window, cx: &mut App) {
    match cx.try_global::<Host>().map(|h| h.open_url) {
        Some(open) => open(url, window, cx),
        None => cx.open_url(url),
    }
}

/// A plugin: the kind of tab it adds.
pub struct Plugin {
    /// Its tabs' surface type, e.g. `gitlab` (`abromux rpc surface.create '{"type":"gitlab"}'`).
    pub id: &'static str,
    /// The command palette's "New … Tab" name.
    pub title: &'static str,
    /// A new tab from what [`Tab::save`] gave (or `{}`, or the socket's params).
    pub open: fn(&Value, &mut Window, &mut App) -> anyhow::Result<TabHandle>,
    /// Files abromux serves to gpui's asset loading, e.g. icons for
    /// `svg().path(…)`: (path, bytes). Paths start with `plugins/<id>/`.
    pub assets: &'static [(&'static str, &'static [u8])],
    /// Web links it shows itself: a ⌘-clicked or opened link it claims
    /// opens as its tab (`open` with `{"url": …}`) instead of a browser.
    pub claims: fn(&str) -> bool,
    /// A command palette entry that asks for one of those links and opens
    /// it, e.g. "Open GitLab Merge Request…" (None: no such entry).
    pub open_link: Option<&'static str>,
}

/// A plugin tab, as abromux holds it.
#[derive(Clone)]
pub struct TabHandle {
    plugin: &'static str,
    view: AnyView,
    tab: Rc<dyn ErasedTab>,
    /// The name from `rename-tab`, kept by abromux.
    name: Rc<RefCell<Option<String>>>,
}

impl TabHandle {
    pub fn new<T: Tab>(plugin: &'static str, view: Entity<T>) -> Self {
        Self {
            plugin,
            view: view.clone().into(),
            tab: Rc::new(view),
            name: Rc::default(),
        }
    }

    pub fn plugin(&self) -> &'static str {
        self.plugin
    }

    pub fn view(&self) -> AnyView {
        self.view.clone()
    }

    pub fn entity_id(&self) -> EntityId {
        self.view.entity_id()
    }

    pub fn title(&self, cx: &App) -> String {
        self.tab.title(cx)
    }

    pub fn icon(&self, cx: &App) -> &'static str {
        self.tab.icon(cx)
    }

    pub fn save(&self, cx: &App) -> Value {
        self.tab.save(cx)
    }

    pub fn state(&self, cx: &App) -> Value {
        self.tab.state(cx)
    }

    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.tab.focus_handle(cx)
    }

    pub fn command(
        &self,
        name: &str,
        args: &Value,
        window: &mut Window,
        cx: &mut App,
    ) -> anyhow::Result<Value> {
        self.tab.command(name, args, window, cx)
    }

    pub fn custom_title(&self) -> Option<String> {
        self.name.borrow().clone()
    }

    pub fn set_custom_title(&self, title: Option<String>, cx: &mut App) {
        *self.name.borrow_mut() = title;
        cx.notify(self.entity_id());
    }
}

/// [`Tab`] without its type, so abromux can hold any plugin's.
trait ErasedTab {
    fn title(&self, cx: &App) -> String;
    fn icon(&self, cx: &App) -> &'static str;
    fn save(&self, cx: &App) -> Value;
    fn state(&self, cx: &App) -> Value;
    fn focus_handle(&self, cx: &App) -> FocusHandle;
    fn command(&self, name: &str, args: &Value, window: &mut Window, cx: &mut App)
    -> anyhow::Result<Value>;
}

impl<T: Tab> ErasedTab for Entity<T> {
    fn title(&self, cx: &App) -> String {
        self.read(cx).title(cx)
    }
    fn icon(&self, cx: &App) -> &'static str {
        self.read(cx).icon()
    }
    fn save(&self, cx: &App) -> Value {
        self.read(cx).save(cx)
    }
    fn state(&self, cx: &App) -> Value {
        self.read(cx).state(cx)
    }
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.read(cx).focus_handle(cx)
    }
    fn command(
        &self,
        name: &str,
        args: &Value,
        window: &mut Window,
        cx: &mut App,
    ) -> anyhow::Result<Value> {
        self.update(cx, |t, cx| t.command(name, args, window, cx))
    }
}
