//! The page in a browser pane, behind one trait, so the pane doesn't care
//! what shows it. macOS has `wkwebview` (WKWebView through wry); other
//! systems have `none`, which says there's no built-in browser.

use futures::channel::{mpsc, oneshot};
use gpui::{AnyElement, App};
use http::{Request, Response};
use std::{borrow::Cow, path::PathBuf};

#[cfg(not(target_os = "macos"))]
pub mod none;
#[cfg(target_os = "macos")]
pub mod wkwebview;

/// A script's JSON result, or why it didn't run.
pub type Answer = Result<String, String>;

/// A page bitmap: premultiplied BGRA, width, height.
pub type Pixels = (Vec<u8>, u32, u32);

/// Serves one URL scheme to the page (Markdown tabs' `abromux-md://`).
pub type Protocol = Box<dyn Fn(&Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> + 'static>;

/// Decides each navigation: true lets it happen.
pub type Navigation = Box<dyn Fn(String) -> bool + 'static>;

/// What a new page starts with.
pub struct PageOptions {
    /// The first URL; empty for a blank page.
    pub url: String,
    /// A blank page's background (0xRRGGBB), rather than white.
    pub blank_bg: u32,
    /// Scripts run in the top frame of every document, before the page's own.
    pub init_scripts: Vec<&'static str>,
    /// The page script run in every frame; its messages arrive as
    /// [`PageEvent::FrameMessage`].
    pub frame_script: &'static str,
    /// The page side of the passkey bridge.
    pub webauthn_script: &'static str,
    /// A URL scheme this page serves itself, and what serves it.
    pub protocol: Option<(&'static str, Protocol)>,
    /// Asked about each navigation; None lets them all happen.
    pub navigation: Option<Navigation>,
    /// Files dropped on the page come as [`PageEvent::FilesDropped`]
    /// instead of reaching the page.
    pub file_drops: bool,
}

/// What a page reports, in the order it happens.
pub enum PageEvent {
    Title(String),
    /// A load started, or `finished`.
    Url {
        url: String,
        finished: bool,
    },
    /// A `window.ipc.postMessage` body from the top frame.
    Ipc(String),
    /// A message from the frame script, in any frame.
    FrameMessage(String),
    DownloadStarted {
        url: String,
        path: PathBuf,
    },
    DownloadDone {
        url: String,
        ok: bool,
    },
    /// Files dropped on the page (with [`PageOptions::file_drops`]).
    FilesDropped(Vec<PathBuf>),
}

/// One page. Everything runs on the main thread.
pub trait PageBackend {
    /// The page's place in the pane's layout.
    fn element(&self) -> AnyElement;
    /// Put the page on screen or take it off (native views draw above gpui,
    /// so a hidden pane's page must be hidden explicitly).
    fn set_shown(&self, shown: bool, cx: &mut App);

    fn load_url(&self, url: &str, cx: &mut App);
    fn reload(&self, cx: &App);
    fn stop(&self);
    fn is_loading(&self) -> bool;
    /// Page zoom, 1.0 at actual size.
    fn set_zoom(&self, zoom: f64);

    /// Run `js` in the top frame and don't wait for it.
    fn run(&self, js: &str, cx: &App);
    /// Run `js` in the top frame; answers with its JSON-encoded result.
    fn eval(&self, js: &str, cx: &App) -> oneshot::Receiver<Answer>;
    /// Run `js` in the frame whose frame script made `token`.
    fn eval_in_frame(&self, token: &str, js: &str) -> oneshot::Receiver<Answer>;
    /// Whether the frame that made `token` is still there.
    fn frame_known(&self, token: &str) -> bool;

    /// The back/forward list, as text for the session file.
    fn history_state(&self) -> Option<String>;
    /// Bring back a [`history_state`](Self::history_state), which also loads
    /// its current page. False if the page didn't take it.
    fn restore_history(&self, state: &str) -> bool;

    /// Give the page keyboard focus.
    fn focus(&self, cx: &App);
    /// Take keyboard focus back from the page, to the window.
    fn unfocus(&self, cx: &App);
    /// Whether the page holds keyboard focus.
    fn has_focus(&self) -> bool;

    /// The page as shown, to stand in for it under the app's popovers. None
    /// if there's none to give.
    fn bitmap(&self) -> oneshot::Receiver<Option<Pixels>>;
    /// The page as shown, as PNG, for `browser screenshot`.
    fn png(&self) -> oneshot::Receiver<Option<Vec<u8>>>;

    /// Every cookie, as a JSON array.
    fn cookies(&self) -> oneshot::Receiver<Answer>;
    /// Set the cookies in a JSON array; answers with how many.
    fn set_cookies(&self, json: &str) -> oneshot::Receiver<Answer>;
    /// Delete the cookies a JSON array of {name, domain, path} names;
    /// answers with how many.
    fn delete_cookies(&self, json: &str) -> oneshot::Receiver<Answer>;

    /// Show the link under the pointer at the page's corner; "" hides it.
    fn hover(&self, url: &str);
    /// The page inspector, or its console.
    fn show_inspector(&self, console: bool);
}

/// A new page on this OS's backend. Its events go to `events`.
pub fn new_page(
    options: PageOptions,
    events: mpsc::UnboundedSender<PageEvent>,
    window: &mut gpui::Window,
    cx: &mut App,
) -> Box<dyn PageBackend> {
    #[cfg(target_os = "macos")]
    return Box::new(wkwebview::WkPage::new(options, events, window, cx));
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (window, cx);
        Box::new(none::NoPage::new(options, events))
    }
}

/// Yields after each left click in the app: pages are native views, so gpui
/// never sees clicks into them.
pub fn watch_clicks() -> mpsc::UnboundedReceiver<()> {
    #[cfg(target_os = "macos")]
    return wkwebview::watch_clicks();
    #[cfg(not(target_os = "macos"))]
    mpsc::unbounded().1
}
