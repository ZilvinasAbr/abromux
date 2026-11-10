//! The plugins built in (crates/, each behind a Cargo feature). Each adds a
//! kind of tab, `Surface::Plugin`; see crates/abromux-plugin.

pub use abromux_plugin::{Plugin, TabHandle};
use gpui::{App, Global, WeakEntity, Window};

/// The app window's view, for what plugins ask of it.
struct Shell(WeakEntity<crate::AbroMux>);

impl Global for Shell {}

/// Answer plugins' requests (`abromux_plugin::Host`) with `view`.
pub fn set_host(view: WeakEntity<crate::AbroMux>, cx: &mut App) {
    cx.set_global(Shell(view));
    cx.set_global(abromux_plugin::Host { open_url, run_terminal, jira });
}

/// A plugin's command in a terminal (a local diff's agents): deferred, as
/// `open_url` is, so the app's view is free to add the tab.
fn run_terminal(run: abromux_plugin::Run, window: &mut Window, cx: &mut App) {
    let Some(view) = cx.try_global::<Shell>().and_then(|s| s.0.upgrade()) else { return };
    window.defer(cx, move |window, cx| {
        view.update(cx, |this, cx| {
            if let Err(e) = this.run_beside(run, window, cx) {
                eprintln!("abromux: a plugin's terminal: {e:#}");
            }
        });
    });
}

/// The Jira a plugin's ticket keys link to (`links::jira_base`).
fn jira(cx: &App) -> Option<String> {
    let view = cx.try_global::<Shell>().and_then(|s| s.0.upgrade())?;
    view.read(cx).jira_base(cx)
}

/// Plugin `id` is in use: GitLab's can be turned off (`gitlab.enabled`).
pub fn enabled(id: &str, cx: &App) -> bool {
    id != "gitlab" || cx.global::<crate::settings::Settings>().gitlab.enabled
}

/// The plugin that shows `url` itself, if one that's in use does.
pub fn claiming(url: &str, cx: &App) -> Option<&'static Plugin> {
    all().iter().find(|p| enabled(p.id, cx) && (p.claims)(url))
}

/// A plugin's link: in a browser beside the focused tab, as a terminal's
/// ⌘-click does.
fn open_url(url: &str, window: &mut Window, cx: &mut App) {
    let view = cx.try_global::<Shell>().and_then(|s| s.0.upgrade());
    let Some(view) = view else {
        cx.open_url(url);
        return;
    };
    let url = url.to_string();
    // Deferred: the plugin's tab is being updated now, and the app's view
    // must be free to add a tab.
    window.defer(cx, move |window, cx| {
        view.update(cx, |this, cx| {
            match this.ws().focused_surface().map(|s| s.entity_id()) {
                Some(id) => this.open_link(id, crate::link::Target::Url(url), window, cx),
                None => cx.open_url(&url),
            }
        });
    });
}

pub fn all() -> &'static [Plugin] {
    &[
        #[cfg(feature = "gitlab")]
        abromux_gitlab::PLUGIN,
    ]
}

pub fn find(id: &str) -> Option<&'static Plugin> {
    all().iter().find(|p| p.id == id)
}

/// A plugin's asset (`Plugin::assets`), by path.
pub fn asset(path: &str) -> Option<&'static [u8]> {
    all()
        .iter()
        .flat_map(|p| p.assets)
        .find_map(|(p, bytes)| (*p == path).then_some(*bytes))
}
