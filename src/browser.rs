#![allow(unexpected_cfgs)]
//! A browser pane: address bar + native WKWebView (via wry / gpui-component).

use crate::{
    browser_find,
    browser_history::{self, Entry},
    browser_native::{self, Pixels},
    downloads::{self, Downloads},
    find_bar::{FindBar, Step},
    markdown,
    terminal::{Find, FindNext, FindPrevious, UseSelectionForFind},
    theme::*,
    webauthn,
};
use futures::{
    StreamExt,
    channel::{mpsc, oneshot},
};
use gpui::{prelude::*, *};
use gpui_component::{
    input::{self, Input, InputEvent, InputState},
    webview::WebView,
    wry::{self, WebViewExtMacOS},
};
use objc::{msg_send, runtime::Object, sel, sel_impl};
use serde_json::Value;
use std::{
    collections::{HashMap, VecDeque},
    f32::consts::PI,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, SystemTime},
};

actions!(
    browser,
    [
        Back,
        Forward,
        Reload,
        ToggleDevTools,
        ShowConsole,
        SuggestNext,
        SuggestPrev,
        DismissSuggestions,
        RemoveSuggestion,
        ZoomIn,
        ZoomOut,
        ActualSize,
        ToggleDownloads
    ]
);

/// Key context of the address bar, for its bindings in main.rs.
pub const OMNIBAR: &str = "Omnibar";
const MAX_SUGGESTIONS: usize = 8;
/// Console messages, errors and requests kept per pane, each.
const LOG_CAP: usize = 1000;
/// How long a popover waits for the page's bitmap before it hides the page
/// anyway.
const COVER_WAIT: Duration = Duration::from_millis(150);

/// Page zoom steps for ⌘= / ⌘-, Safari's.
const ZOOM_LEVELS: [f64; 11] = [0.5, 0.75, 0.85, 1.0, 1.15, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0];

/// The zoom step after `zoom` going in or out, or `zoom` at either end.
pub(crate) fn zoom_step(zoom: f64, zoom_in: bool) -> f64 {
    const EPS: f64 = 0.001;
    if zoom_in {
        ZOOM_LEVELS.iter().copied().find(|&z| z > zoom + EPS)
    } else {
        ZOOM_LEVELS.iter().rev().copied().find(|&z| z < zoom - EPS)
    }
    .unwrap_or(zoom)
}

/// A toolbar button, indexing `BrowserView::nav_presses`.
#[derive(Clone, Copy, PartialEq)]
enum Nav {
    Back,
    Forward,
    Reload,
}

pub enum BrowserEvent {
    TitleChanged,
    Focused,
    /// ⌘- or middle-click on a link; `focus` (⇧ held) switches to the new tab.
    OpenTab {
        url: String,
        focus: bool,
    },
}

enum WebMsg {
    Title(String),
    /// A page load started, or `finished`.
    Url {
        url: String,
        finished: bool,
    },
    /// A find script's result, or the page selection for ⌘E.
    Find(String),
    Selection(String),
    OpenTab {
        url: String,
        focus: bool,
    },
    DownloadStarted {
        url: String,
        path: PathBuf,
    },
    DownloadDone {
        url: String,
        ok: bool,
    },
    /// The page's bitmap for cover `generation`.
    Cover {
        generation: u64,
        pixels: Option<Pixels>,
    },
    /// A Markdown tab's `mailto:` link: the system's.
    External(String),
}

/// What the page has logged, for `browser console`, `errors` and `network`.
/// Each entry is the page script's JSON plus the frame it came from.
#[derive(Default)]
pub struct PageLog {
    pub console: VecDeque<Value>,
    pub errors: VecDeque<Value>,
    pub network: VecDeque<Value>,
}

impl PageLog {
    fn push(&mut self, mut item: Value, frame: Option<&str>) {
        let list = match item["type"].as_str() {
            Some("console") => &mut self.console,
            Some("error") => &mut self.errors,
            Some("network") => &mut self.network,
            _ => return,
        };
        if let Some(f) = frame {
            item["frame"] = Value::String(f.to_string());
        }
        if list.len() >= LOG_CAP {
            list.pop_front();
        }
        list.push_back(item);
    }
}

/// The page under a popover: native views draw above gpui, so the page
/// hides and a bitmap of it stands in while something overlaps it.
enum Cover {
    None,
    /// Waiting for the bitmap; the page still shows (`hidden`: no longer).
    Pending {
        generation: u64,
        hidden: bool,
    },
    /// The bitmap, or None if WebKit had none to give.
    Shown(Option<Arc<RenderImage>>),
}

/// Page script for ⌘-click / middle-click on links.
const LINKS_SCRIPT: &str = include_str!("browser_links.js");

pub struct BrowserView {
    webview: Entity<WebView>,
    address: Entity<InputState>,
    pub title: String,
    pub url: String,
    /// Set by `rename-tab`; shown instead of the page title.
    pub custom_title: Option<String>,
    focus: FocusHandle,
    shown: bool,
    tx: mpsc::UnboundedSender<WebMsg>,
    /// The ⌘F bar.
    find: FindBar,
    /// Presses per toolbar button; each new one replays its animation.
    /// Reload's counts swaps between its reload and stop icons instead.
    nav_presses: [usize; 3],
    /// WKWebView's `isLoading`, while `watch_loading` polls it.
    loading: bool,
    polling: bool,
    /// History matching the address bar's text, best first, while it's
    /// being edited. The page hides while there are any, since it draws
    /// above gpui.
    suggestions: Vec<Entry>,
    /// The highlighted suggestion; None leaves Return to the typed text.
    selected: Option<usize>,
    /// Whether the native webview is on screen: `shown` and no suggestions.
    native_shown: bool,
    /// The title the loading page set, for its history entry.
    load_title: Option<String>,
    /// WKWebView's `pageZoom`, 1.0 at actual size.
    zoom: f64,
    /// Something overlaps the page (a popover, the palette, a menu).
    covered: bool,
    cover: Cover,
    cover_generation: u64,
    /// Bitmaps to free from the GPU at the next render.
    stale_covers: Vec<Arc<RenderImage>>,
    pub log: PageLog,
    /// The frame `browser frame` picked (its page script's token) and its
    /// URL; None for the page itself.
    pub frame: Option<(String, String)>,
    /// `browser frame`'s questions to iframes, by nonce, waiting for their token.
    frame_asks: HashMap<String, oneshot::Sender<String>>,
    downloads: Entity<Downloads>,
    downloads_open: bool,
    /// When a click outside closed the list, so the same click on the
    /// button doesn't open it again.
    downloads_closed_at: Option<std::time::Instant>,
    /// A Markdown tab (src/markdown.rs): it shows local files only, and
    /// follows the shown one as it changes.
    pub markdown: bool,
    /// The file it last looked at and when that changed.
    md_seen: Option<(PathBuf, Option<SystemTime>)>,
}

impl EventEmitter<BrowserEvent> for BrowserView {}

impl Focusable for BrowserView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl BrowserView {
    pub fn new(url: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (tx, mut rx) = mpsc::unbounded::<WebMsg>();
        let (tx_title, tx_url, tx_ipc) = (tx.clone(), tx.clone(), tx.clone());
        let (tx_dl_start, tx_dl_done, tx_nav) = (tx.clone(), tx.clone(), tx.clone());
        let is_markdown = markdown::is_markdown_url(url);
        let builder = wry::WebViewBuilder::new();
        // A blank page gets the pane's background rather than white. A URL
        // loads once the WebAuthn bridge is in, so its first page has it.
        let builder = if url.is_empty() {
            builder.with_html(format!(
                "<html style=\"background:#{:06x}\"></html>",
                pal().pane_bg
            ))
        } else {
            builder
        };
        let raw = builder
            .with_user_agent(user_agent())
            .with_devtools(true)
            .with_document_title_changed_handler(move |t| {
                let _ = tx_title.unbounded_send(WebMsg::Title(t));
            })
            .with_on_page_load_handler(move |ev, url| {
                let finished = matches!(ev, wry::PageLoadEvent::Finished);
                let _ = tx_url.unbounded_send(WebMsg::Url { url, finished });
            })
            .with_initialization_script(LINKS_SCRIPT)
            // WebKit saves it where wry says (a free name in ~/Downloads).
            .with_download_started_handler(move |url, path| {
                let _ = tx_dl_start.unbounded_send(WebMsg::DownloadStarted {
                    url,
                    path: path.clone(),
                });
                true
            })
            .with_download_completed_handler(move |url, _, ok| {
                let _ = tx_dl_done.unbounded_send(WebMsg::DownloadDone { url, ok });
            })
            .with_ipc_handler(move |req| {
                #[derive(serde::Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct Msg {
                    open_tab: String,
                    #[serde(default)]
                    focus: bool,
                }
                let Ok(Msg {
                    open_tab: url,
                    focus,
                }) = serde_json::from_str(req.body())
                else {
                    return;
                };
                if url.starts_with("http://") || url.starts_with("https://") {
                    let _ = tx_ipc.unbounded_send(WebMsg::OpenTab { url, focus });
                }
            });
        // Only Markdown tabs can load the scheme, and they load nothing else:
        // web links open as browser tabs beside them.
        let raw = if is_markdown {
            raw.with_custom_protocol(markdown::SCHEME.into(), |_, req| markdown::respond(&req))
                .with_navigation_handler(move |url| {
                    if markdown::is_markdown_url(&url) || url.starts_with("about:") {
                        return true;
                    }
                    let msg = if url.starts_with("http://") || url.starts_with("https://") {
                        WebMsg::OpenTab { url, focus: true }
                    } else if url.starts_with("mailto:") {
                        WebMsg::External(url)
                    } else {
                        return false;
                    };
                    let _ = tx_nav.unbounded_send(msg);
                    false
                })
        } else {
            raw
        };
        let raw = raw.build_as_child(window).expect("create webview");
        webauthn::install(&raw);
        let (page_tx, mut page_rx) = mpsc::unbounded::<String>();
        {
            let wk = raw.webview();
            browser_native::install(&*wk as *const _ as *mut Object, page_tx);
        }
        // wry's delegate class exists now that a webview does.
        browser_native::downloads_install();
        browser_native::page_shortcuts_install();
        // NSViews don't clip by default since macOS 14; keep the page from
        // drawing past its frame over the focused pane's border just outside.
        {
            let wk = raw.webview();
            let wk = &*wk as *const _ as *mut Object;
            let _: () = unsafe { msg_send![wk, setClipsToBounds: true] };
        }
        crate::cursor::install();
        if !url.is_empty() {
            let _ = raw.load_url(url);
        }
        let webview = cx.new(|cx| WebView::new(raw, window, cx));

        let address = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Search or enter URL")
                .default_value(url.to_string())
        });
        cx.subscribe_in(
            &address,
            window,
            |this, input, ev: &InputEvent, window, cx| match ev {
                InputEvent::PressEnter { .. } => {
                    let text = input.read(cx).value().trim().to_string();
                    let picked = this.selected.and_then(|i| this.suggestions.get(i));
                    let url = match picked {
                        Some(e) => e.url.clone(),
                        None if text.is_empty() => return,
                        None => normalize_url(&text),
                    };
                    let picked = picked.is_some();
                    // A search isn't a URL the user would type again.
                    if picked || !is_search(&url) {
                        browser_history::typed(&url);
                    }
                    if picked {
                        this.address
                            .update(cx, |a, cx| a.set_value(url.clone(), window, cx));
                    }
                    this.set_suggestions(vec![], cx);
                    this.navigate(&url, cx);
                    this.focus_page(window, cx);
                }
                InputEvent::Change => {
                    if input.read(cx).focus_handle(cx).is_focused(window) {
                        this.update_suggestions(cx);
                    }
                }
                InputEvent::Focus => cx.emit(BrowserEvent::Focused),
                InputEvent::Blur => this.set_suggestions(vec![], cx),
            },
        )
        .detach();

        cx.spawn_in(window, async move |this, cx| {
            while let Some(msg) = rx.next().await {
                let ok = this.update_in(cx, |this, window, cx| {
                    match msg {
                        WebMsg::Title(t) => {
                            if !this.markdown {
                                browser_history::retitle(&this.url, &t);
                            }
                            this.load_title = Some(t.clone());
                            this.title = t;
                        }
                        WebMsg::Url { url: u, finished } => {
                            let u = if u.starts_with("about:blank") {
                                String::new()
                            } else {
                                u
                            };
                            if finished && !this.markdown {
                                browser_history::visit(&u, this.load_title.as_deref());
                            } else {
                                this.load_title = None;
                            }
                            this.url = u.clone();
                            this.watch_loading(cx);
                            this.address.update(cx, |a, cx| a.set_value(u, window, cx));
                            // A new document has no highlights; search it afresh.
                            this.run_find(cx);
                        }
                        WebMsg::Find(json) => {
                            this.find.count = browser_find::parse_count(&json).unwrap_or_default();
                        }
                        WebMsg::Selection(json) => {
                            let text: String = serde_json::from_str(&json).unwrap_or_default();
                            let text = text
                                .lines()
                                .map(str::trim)
                                .find(|l| !l.is_empty())
                                .unwrap_or_default()
                                .to_string();
                            if !text.is_empty() {
                                this.open_find(window, cx);
                                if let Some(input) = &this.find.input {
                                    input.update(cx, |i, cx| i.set_value(text, window, cx));
                                }
                                this.run_find(cx);
                            }
                        }
                        WebMsg::OpenTab { url, focus } => {
                            cx.emit(BrowserEvent::OpenTab { url, focus });
                            return;
                        }
                        WebMsg::DownloadStarted { url, path } => {
                            this.downloads.update(cx, |d, cx| d.started(url, path, cx));
                            return;
                        }
                        WebMsg::DownloadDone { url, ok } => {
                            this.downloads.update(cx, |d, cx| d.finished(&url, ok, cx));
                            return;
                        }
                        WebMsg::Cover { generation, pixels } => {
                            this.cover_arrived(generation, pixels, cx);
                            return;
                        }
                        WebMsg::External(url) => {
                            cx.open_url(&url);
                            return;
                        }
                    }
                    cx.emit(BrowserEvent::TitleChanged);
                    cx.notify();
                });
                if ok.is_err() {
                    break;
                }
            }
        })
        .detach();

        cx.spawn(async move |this, cx| {
            while let Some(json) = page_rx.next().await {
                let Ok(msg) = serde_json::from_str::<Value>(&json) else {
                    continue;
                };
                if this
                    .update(cx, |this, cx| this.page_message(msg, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        let downloads = downloads::entity(cx);
        cx.observe(&downloads, |_, _, cx| cx.notify()).detach();
        if is_markdown {
            Self::follow_markdown(cx);
        }

        Self {
            webview,
            address,
            title: String::new(),
            url: url.into(),
            custom_title: None,
            focus: cx.focus_handle(),
            shown: true,
            tx,
            find: FindBar::default(),
            nav_presses: [0; 3],
            loading: false,
            polling: false,
            suggestions: vec![],
            selected: None,
            native_shown: true,
            load_title: None,
            zoom: 1.0,
            covered: false,
            cover: Cover::None,
            cover_generation: 0,
            stale_covers: vec![],
            log: PageLog::default(),
            frame: None,
            frame_asks: HashMap::new(),
            downloads,
            downloads_open: false,
            downloads_closed_at: None,
            markdown: is_markdown,
            md_seen: None,
        }
    }

    /// A Markdown tab's live reload: while it's on screen, check the shown
    /// file twice a second and put its new content in place when it changes
    /// (without a reload, so the scroll position stays).
    fn follow_markdown(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(500))
                    .await;
                let alive = this.update(cx, |this, cx| {
                    if !this.shown {
                        return;
                    }
                    let Some(path) = markdown::path_of(&this.url) else {
                        return;
                    };
                    let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
                    match &this.md_seen {
                        // A page just navigated to is already current.
                        Some((seen, _)) if *seen != path => {}
                        Some((_, seen)) if *seen != mtime => {
                            if markdown::is_markdown(&path) {
                                this.script(&markdown::reload_script(&path), cx);
                            } else {
                                this.reload(cx);
                            }
                        }
                        Some(_) => return,
                        None => {}
                    }
                    this.md_seen = Some((path, mtime));
                });
                if alive.is_err() {
                    return;
                }
            }
        })
        .detach();
    }

    /// A message from the page script (browser_page.js) in any frame.
    fn page_message(&mut self, msg: Value, cx: &mut Context<Self>) {
        let main = msg["main"].as_bool().unwrap_or(true);
        let frame = if main { None } else { msg["frameUrl"].as_str() };
        match msg["kind"].as_str() {
            Some("hello") => {
                // A new document in the page drops its frames, the picked one too.
                if main {
                    self.frame = None;
                    self.frame_asks.clear();
                    browser_native::hover(self.wk_webview(cx), "");
                }
            }
            Some("answer") => {
                if let (Some(nonce), Some(token)) = (msg["nonce"].as_str(), msg["token"].as_str())
                    && let Some(tx) = self.frame_asks.remove(nonce)
                {
                    let _ = tx.send(token.to_string());
                }
            }
            Some("events") => {
                if let Some(items) = msg["items"].as_array() {
                    for item in items {
                        self.log.push(item.clone(), frame);
                    }
                }
            }
            Some("hover") => {
                let url = msg["url"].as_str().unwrap_or("");
                browser_native::hover(self.wk_webview(cx), url);
            }
            _ => {}
        }
    }

    pub fn navigate(&mut self, url: &str, cx: &mut Context<Self>) {
        self.url = url.into();
        self.webview.update(cx, |w, _| w.load_url(url));
        self.watch_loading(cx);
    }

    pub fn focus_address(this: &Entity<Self>, window: &mut Window, cx: &mut App) {
        // A Markdown tab has no address bar.
        if this.read(cx).markdown {
            let _ = this.read(cx).webview.read(cx).raw().focus();
            this.update(cx, |this, cx| this.page_focused(window, cx));
            return;
        }
        let this = this.read(cx);
        // Take native focus back from the page so typing reaches the address bar.
        let _ = this.webview.read(cx).raw().focus_parent();
        let address = this.address.clone();
        address.update(cx, |a, cx| a.focus(window, cx));
    }

    /// Give the page keyboard focus, natively and in gpui.
    fn focus_page(&self, window: &mut Window, cx: &mut App) {
        let _ = self.webview.read(cx).raw().focus();
        self.page_focused(window, cx);
    }

    /// Follow the page taking native focus: move gpui focus off the address
    /// bar (or it keeps taking ⌘A/⌘C/⌘V) and drop its selection, which the
    /// input otherwise keeps showing.
    pub fn page_focused(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.focus);
        self.address.update(cx, |a, cx| a.unselect(window, cx));
    }

    /// Whether the page holds native keyboard focus (the window's first
    /// responder is the WKWebView or a view inside it).
    pub fn page_has_focus(&self, cx: &App) -> bool {
        let webview = self.webview.read(cx).raw().webview();
        let webview = &*webview as *const _ as *mut Object;
        unsafe {
            let window: *mut Object = msg_send![webview, window];
            if window.is_null() {
                return false;
            }
            let responder: *mut Object = msg_send![window, firstResponder];
            let is_view: bool =
                !responder.is_null() && msg_send![responder, isKindOfClass: objc::class!(NSView)];
            is_view && msg_send![responder, isDescendantOf: webview]
        }
    }

    /// Native views float above gpui content, so hidden panes must be hidden
    /// explicitly.
    pub fn set_shown(&mut self, shown: bool, cx: &mut Context<Self>) {
        self.set_visibility(shown, false, cx);
    }

    /// Whether the pane is on screen, and whether something of the app's
    /// (a popover, the palette, a menu) overlaps it. A covered page shows
    /// as a bitmap of itself, which gpui can draw things over.
    pub fn set_visibility(&mut self, shown: bool, covered: bool, cx: &mut Context<Self>) {
        if (self.shown, self.covered) != (shown, covered) {
            self.shown = shown;
            self.covered = covered;
            self.sync_native(cx);
        }
    }

    fn sync_native(&mut self, cx: &mut Context<Self>) {
        let covered =
            self.shown && (self.covered || !self.suggestions.is_empty() || self.downloads_open);
        let native = if !covered {
            if let Cover::Shown(Some(img)) = std::mem::replace(&mut self.cover, Cover::None) {
                self.stale_covers.push(img);
            }
            self.cover = Cover::None;
            self.shown
        } else {
            match self.cover {
                Cover::Shown(_) => false,
                Cover::Pending { hidden, .. } => !hidden,
                Cover::None => {
                    self.take_cover(cx);
                    true
                }
            }
        };
        if self.native_shown != native {
            self.native_shown = native;
            if !native {
                browser_native::hover(self.wk_webview(cx), "");
            }
            self.webview
                .update(cx, |w, _| if native { w.show() } else { w.hide() });
        }
        cx.notify();
    }

    /// Snapshot the page to stand in for it. The page stays until the bitmap
    /// comes (or `COVER_WAIT` passes), so there's no blank frame.
    fn take_cover(&mut self, cx: &mut Context<Self>) {
        self.cover_generation += 1;
        let generation = self.cover_generation;
        self.cover = Cover::Pending {
            generation,
            hidden: false,
        };
        let rx = browser_native::snapshot(self.wk_webview(cx));
        let tx = self.tx.clone();
        cx.foreground_executor()
            .spawn(async move {
                let pixels = rx.await.ok().flatten();
                let _ = tx.unbounded_send(WebMsg::Cover { generation, pixels });
            })
            .detach();
        cx.spawn(async move |this, cx| {
            Timer::after(COVER_WAIT).await;
            let _ = this.update(cx, |this, cx| {
                if let Cover::Pending {
                    generation: g,
                    hidden,
                } = &mut this.cover
                    && *g == generation
                {
                    *hidden = true;
                    this.sync_native_shown(false, cx);
                }
            });
        })
        .detach();
    }

    fn sync_native_shown(&mut self, native: bool, cx: &mut Context<Self>) {
        if self.native_shown != native {
            self.native_shown = native;
            self.webview
                .update(cx, |w, _| if native { w.show() } else { w.hide() });
            cx.notify();
        }
    }

    fn cover_arrived(&mut self, generation: u64, pixels: Option<Pixels>, cx: &mut Context<Self>) {
        let Cover::Pending { generation: g, .. } = self.cover else {
            return;
        };
        if g != generation {
            return;
        }
        let img = pixels.and_then(|(bgra, w, h)| {
            let buf = image::RgbaImage::from_raw(w, h, bgra)?;
            Some(Arc::new(RenderImage::new(vec![image::Frame::new(buf)])))
        });
        self.cover = Cover::Shown(img);
        self.sync_native_shown(false, cx);
    }

    fn set_suggestions(&mut self, suggestions: Vec<Entry>, cx: &mut Context<Self>) {
        if suggestions.is_empty() && self.suggestions.is_empty() {
            return;
        }
        self.suggestions = suggestions;
        self.selected = None;
        self.sync_native(cx);
        cx.notify();
    }

    /// Suggest history for the address bar's text, unless it's the page's
    /// own URL (as when a load sets it).
    fn update_suggestions(&mut self, cx: &mut Context<Self>) {
        let text = self.address.read(cx).value().trim().to_string();
        let list = if text.is_empty() || text == self.url {
            vec![]
        } else {
            browser_history::suggest(&text, MAX_SUGGESTIONS)
        };
        self.set_suggestions(list, cx);
    }

    /// Down/Up in the address bar. Down opens the list if it's closed (the
    /// most typed and recent URLs, for the page's own URL or no text); Up
    /// past the first goes back to the typed text.
    fn move_suggestion(&mut self, down: bool, cx: &mut Context<Self>) {
        if self.suggestions.is_empty() {
            if down {
                let text = self.address.read(cx).value().trim().to_string();
                let query = if text == self.url { "" } else { text.as_str() };
                self.set_suggestions(browser_history::suggest(query, MAX_SUGGESTIONS), cx);
            }
            return;
        }
        let last = self.suggestions.len() - 1;
        self.selected = match (self.selected, down) {
            (None, true) => Some(0),
            (Some(i), true) => Some((i + 1).min(last)),
            (Some(0) | None, false) => None,
            (Some(i), false) => Some(i - 1),
        };
        cx.notify();
    }

    /// Esc in the address bar closes the suggestions, or else puts back
    /// the page's URL.
    fn dismiss_suggestions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.suggestions.is_empty() {
            self.set_suggestions(vec![], cx);
        } else {
            let url = self.url.clone();
            self.address
                .update(cx, |a, cx| a.set_value(url, window, cx));
        }
    }

    /// Shift-Backspace on a highlighted suggestion deletes it from history.
    fn remove_suggestion(&mut self, cx: &mut Context<Self>) {
        let Some(i) = self.selected else {
            cx.propagate();
            return;
        };
        let e = self.suggestions.remove(i);
        browser_history::remove(&e.url);
        self.selected = if self.suggestions.is_empty() {
            None
        } else {
            Some(i.min(self.suggestions.len() - 1))
        };
        self.sync_native(cx);
        cx.notify();
    }

    /// Go to suggestion `i` (clicked).
    fn open_suggestion(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(e) = self.suggestions.get(i) else {
            return;
        };
        let url = e.url.clone();
        browser_history::typed(&url);
        self.address
            .update(cx, |a, cx| a.set_value(url.clone(), window, cx));
        self.set_suggestions(vec![], cx);
        self.navigate(&url, cx);
        self.focus_page(window, cx);
    }

    fn render_suggestions(&self, cx: &mut Context<Self>) -> Option<Div> {
        if self.suggestions.is_empty() {
            return None;
        }
        let rows: Vec<_> = self
            .suggestions
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let selected = self.selected == Some(i);
                let shown_url = e
                    .url
                    .strip_prefix("https://")
                    .unwrap_or(&e.url)
                    .trim_end_matches('/')
                    .to_string();
                let title = e.title.clone().filter(|t| !t.is_empty());
                div()
                    .id(("suggestion", i))
                    .mx(px(6.))
                    .px(px(8.))
                    .py(px(5.))
                    .rounded(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .cursor_pointer()
                    .when(selected, |d| d.bg(tint(0x1a)))
                    .when(!selected, |d| d.hover(|s| s.bg(rgba(pal().hover))))
                    // On mouse down, before the address bar loses focus and
                    // closes the list.
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.open_suggestion(i, window, cx);
                        }),
                    )
                    .child(icon("globe").flex_none().text_color(rgb(pal().muted)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_baseline()
                            .gap(px(8.))
                            .children(title.map(|t| {
                                div()
                                    .flex_shrink()
                                    .min_w(px(80.))
                                    .truncate()
                                    .text_size(px(12.5))
                                    .text_color(rgb(pal().text))
                                    .child(t)
                            }))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(11.5))
                                    .text_color(rgb(pal().muted))
                                    .child(shown_url),
                            ),
                    )
            })
            .collect();
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .py(px(6.))
                .flex()
                .flex_col()
                .gap(px(1.))
                .bg(rgb(pal().tabbar_bg))
                .border_b_1()
                .border_color(rgb(pal().border))
                .children(rows),
        )
    }

    pub fn script(&self, js: &str, cx: &App) {
        let _ = self.webview.read(cx).raw().evaluate_script(js);
    }

    pub fn reload(&self, cx: &App) {
        let _ = self.webview.read(cx).raw().reload();
    }

    /// WKWebView's `interactionState` (its back/forward list), base64, for
    /// the session file.
    pub fn history_state(&self, cx: &App) -> Option<String> {
        unsafe {
            let wk = self.wk_webview(cx);
            let state: *mut Object = msg_send![wk, interactionState];
            if state.is_null() {
                return None;
            }
            let is_data: bool = msg_send![state, isKindOfClass: objc::class!(NSData)];
            if !is_data {
                return None;
            }
            let b64: *mut Object = msg_send![state, base64EncodedStringWithOptions: 0usize];
            let utf8: *const std::ffi::c_char = msg_send![b64, UTF8String];
            (!utf8.is_null()).then(|| {
                std::ffi::CStr::from_ptr(utf8)
                    .to_string_lossy()
                    .into_owned()
            })
        }
    }

    /// Bring back a back/forward list from [`history_state`](Self::history_state),
    /// which also loads its current page. False if `state` isn't one, or
    /// WebKit didn't take it.
    pub fn restore_history(&mut self, state: &str, cx: &mut Context<Self>) -> bool {
        let Ok(c) = std::ffi::CString::new(state) else {
            return false;
        };
        let ok = unsafe {
            let s: *mut Object =
                msg_send![objc::class!(NSString), stringWithUTF8String: c.as_ptr()];
            let data: *mut Object = msg_send![objc::class!(NSData), alloc];
            let data: *mut Object = msg_send![data, initWithBase64EncodedString: s options: 0usize];
            if data.is_null() {
                false
            } else {
                let wk = self.wk_webview(cx);
                let _: () = msg_send![wk, setInteractionState: data];
                let _: () = msg_send![data, release];
                // WebKit ignores data it can't read; then there's no page.
                let list: *mut Object = msg_send![wk, backForwardList];
                let item: *mut Object = msg_send![list, currentItem];
                !item.is_null()
            }
        };
        if ok {
            self.watch_loading(cx);
        }
        ok
    }

    pub fn zoom(&self) -> f64 {
        self.zoom
    }

    /// Set the page zoom (⌘= / ⌘- / ⌘0), kept between the first and last step.
    pub fn set_zoom(&mut self, zoom: f64, cx: &mut Context<Self>) {
        let zoom = zoom.clamp(ZOOM_LEVELS[0], ZOOM_LEVELS[ZOOM_LEVELS.len() - 1]);
        self.zoom = zoom;
        let _: () = unsafe { msg_send![self.wk_webview(cx), setPageZoom: zoom] };
        cx.notify();
    }

    fn stop(&self, cx: &App) {
        let _: () = unsafe { msg_send![self.wk_webview(cx), stopLoading] };
    }

    pub(crate) fn wk_webview(&self, cx: &App) -> *mut Object {
        let webview = self.webview.read(cx).raw().webview();
        &*webview as *const _ as *mut Object
    }

    /// Follow WKWebView's `isLoading` for the reload button's stop icon.
    /// wry's load events miss failed loads and fire only once the server
    /// answers, so this polls from anything that may start a load until a
    /// second passes with none.
    pub fn watch_loading(&mut self, cx: &mut Context<Self>) {
        if self.polling {
            return;
        }
        self.polling = true;
        cx.spawn(async move |this, cx| {
            let mut idle = 0;
            loop {
                let polling = this.update(cx, |this, cx| {
                    let loading: bool = unsafe { msg_send![this.wk_webview(cx), isLoading] };
                    if loading != this.loading {
                        this.loading = loading;
                        this.nav_presses[Nav::Reload as usize] += 1;
                        cx.notify();
                    }
                    idle = if loading { 0 } else { idle + 1 };
                    this.polling = idle < 20;
                    this.polling
                });
                if !polling.unwrap_or(false) {
                    return;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
            }
        })
        .detach();
    }

    /// Run a toolbar button's command (or its shortcut's) and animate the button.
    fn nav(&mut self, nav: Nav, cx: &mut Context<Self>) {
        match nav {
            Nav::Back => self.script("history.back()", cx),
            Nav::Forward => self.script("history.forward()", cx),
            Nav::Reload => self.reload(cx),
        }
        if nav != Nav::Reload {
            self.nav_presses[nav as usize] += 1;
        }
        self.watch_loading(cx);
        cx.notify();
    }

    /// ⌥⌘I opens WebKit's inspector, ⌥⌘C its console.
    fn show_inspector(&self, console: bool, cx: &App) {
        let webview = self.webview.read(cx).raw().webview();
        let webview = &*webview as *const _ as *mut Object;
        unsafe {
            let inspector: *mut Object = msg_send![webview, _inspector];
            if inspector.is_null() {
                return;
            }
            let action = if console {
                sel!(showConsole)
            } else {
                sel!(show)
            };
            let ok: bool = msg_send![inspector, respondsToSelector: action];
            if ok {
                let _: () = msg_send![inspector, performSelector: action];
            }
        }
    }

    /// Run a find script and send its count back to the main thread.
    fn find_script(&self, js: &str, cx: &App) {
        let tx = self.tx.clone();
        let _ = self.eval(js, cx, move |json| {
            let _ = tx.unbounded_send(WebMsg::Find(json));
        });
    }

    /// ⌘F: show the find bar (or focus it) with its last search.
    fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = match &self.find.input {
            Some(input) => input.clone(),
            None => {
                let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find in page"));
                cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, _, cx| match ev {
                    InputEvent::Change => this.run_find(cx),
                    // Return steps down, to the next match, like ⌘G.
                    InputEvent::PressEnter { .. } => this.find_step(Step::Down, cx),
                    InputEvent::Focus => cx.emit(BrowserEvent::Focused),
                    _ => {}
                })
                .detach();
                self.find.input = Some(input.clone());
                input
            }
        };
        if !self.find.open {
            self.find.open = true;
            self.run_find(cx);
        }
        // Take native focus back from the page so typing reaches the field.
        let _ = self.webview.read(cx).raw().focus_parent();
        input.update(cx, |i, cx| i.focus(window, cx));
        // Runs once the input has focus, so retyping replaces the old search.
        window.dispatch_action(Box::new(input::SelectAll), cx);
        cx.notify();
    }

    /// Search the page for the find bar's text, if the bar is open.
    fn run_find(&mut self, cx: &mut Context<Self>) {
        if !self.find.open {
            return;
        }
        let needle = self
            .find
            .input
            .as_ref()
            .map(|i| i.read(cx).value().to_string())
            .unwrap_or_default();
        if needle.is_empty() {
            self.find.count = (0, None);
            self.find_script(&browser_find::clear(), cx);
        } else {
            self.find_script(&browser_find::search(&needle), cx);
        }
        cx.notify();
    }

    /// ⌘G / ⇧⌘G (Return / ⇧Return in the bar): the next or previous match.
    fn find_step(&mut self, step: Step, cx: &mut Context<Self>) {
        if self.find.open {
            self.find_script(&browser_find::step(step == Step::Down), cx);
        }
    }

    /// ⌘E: search for the page's selected text.
    fn find_selection(&mut self, cx: &mut Context<Self>) {
        let tx = self.tx.clone();
        let _ = self.eval(browser_find::SELECTION, cx, move |json| {
            let _ = tx.unbounded_send(WebMsg::Selection(json));
        });
    }

    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find.close();
        self.find_script(&browser_find::clear(), cx);
        self.focus_page(window, cx);
        cx.notify();
    }

    /// The find bar, as a strip above the page.
    fn render_find_bar(&self, input: Entity<InputState>, cx: &mut Context<Self>) -> Stateful<Div> {
        self.find
            .render(
                &input,
                Self::find_step,
                |this, window, cx| this.close_find(window, cx),
                cx,
            )
            .flex_none()
            .border_b_1()
            .border_color(rgb(pal().border))
    }

    /// Run `js` and hand its JSON-encoded result to `done` (on a wry thread).
    pub fn eval(
        &self,
        js: &str,
        cx: &App,
        done: impl Fn(String) + Send + 'static,
    ) -> anyhow::Result<()> {
        Ok(self
            .webview
            .read(cx)
            .raw()
            .evaluate_script_with_callback(js, done)?)
    }

    /// Run `js` where automation commands act: in the frame `browser frame`
    /// picked, else in the page. Err if that frame is gone; canceled if the
    /// page dropped the script.
    pub fn eval_target(&self, js: &str, cx: &App) -> oneshot::Receiver<browser_native::Answer> {
        match &self.frame {
            Some((token, _)) => browser_native::eval_in_frame(self.wk_webview(cx), token, js),
            None => {
                let (tx, rx) = oneshot::channel();
                let tx = Mutex::new(Some(tx));
                let _ = self.eval(js, cx, move |out| {
                    if let Some(tx) = tx.lock().unwrap().take() {
                        let _ = tx.send(Ok(out));
                    }
                });
                rx
            }
        }
    }

    /// Wait for the iframe asked with `nonce` to answer with its token.
    pub fn ask_frame(&mut self, nonce: String) -> oneshot::Receiver<String> {
        let (tx, rx) = oneshot::channel();
        self.frame_asks.insert(nonce, tx);
        rx
    }

    /// Point automation commands at a frame (its token and URL), or back at
    /// the page.
    pub fn set_frame(&mut self, frame: Option<(String, String)>, cx: &App) -> bool {
        if let Some((token, _)) = &frame
            && !browser_native::frame_known(self.wk_webview(cx), token)
        {
            return false;
        }
        self.frame = frame;
        true
    }

    /// Save a PNG of the page as shown (WKWebView's own snapshot) to `path`.
    pub fn screenshot(
        &self,
        path: std::path::PathBuf,
        cx: &App,
    ) -> oneshot::Receiver<anyhow::Result<serde_json::Value>> {
        let (tx, rx) = oneshot::channel();
        let tx = Mutex::new(Some(tx));
        let handler = block::ConcreteBlock::new(move |image: *mut Object, _error: *mut Object| {
            let out = unsafe { png_data(image) }
                .ok_or_else(|| {
                    anyhow::anyhow!("WebKit couldn't snapshot the page (is it on screen?)")
                })
                .and_then(|png| {
                    std::fs::write(&path, &png)
                        .map_err(|e| anyhow::anyhow!("can't write {}: {e}", path.display()))?;
                    Ok(serde_json::json!({ "path": path, "bytes": png.len() }))
                });
            if let Some(tx) = tx.lock().unwrap().take() {
                let _ = tx.send(out);
            }
        })
        .copy();
        let webview = self.webview.read(cx).raw().webview();
        let webview = &*webview as *const _ as *mut Object;
        let nil: *mut Object = std::ptr::null_mut();
        let _: () = unsafe {
            msg_send![webview, takeSnapshotWithConfiguration: nil completionHandler: &*handler]
        };
        rx
    }

    /// A toolbar button. Back and forward nudge their chevron that way and
    /// back; reload turns into stop while the page loads, as in cmux, and
    /// the new icon grows in.
    fn render_nav(&self, nav: Nav, cx: &mut Context<Self>) -> Stateful<Div> {
        let (id, name) = match nav {
            Nav::Back => ("back", "chevron-left"),
            Nav::Forward => ("fwd", "chevron-right"),
            Nav::Reload if self.loading => ("reload", "x"),
            Nav::Reload => ("reload", "rotate-cw"),
        };
        let icon = icon(name).text_color(rgb(pal().muted));
        let presses = self.nav_presses[nav as usize];
        // Nothing plays before the first press.
        let icon = if presses == 0 {
            icon.into_any_element()
        } else {
            let animation = match nav {
                Nav::Reload => {
                    Animation::new(Duration::from_millis(180)).with_easing(ease_out_quint())
                }
                _ => Animation::new(Duration::from_millis(250)),
            };
            icon.with_animation((id, presses), animation, move |icon, t| {
                let nudge = 3. * (PI * t).sin();
                match nav {
                    Nav::Back => icon
                        .with_transformation(Transformation::translate(point(px(-nudge), px(0.)))),
                    Nav::Forward => icon
                        .with_transformation(Transformation::translate(point(px(nudge), px(0.)))),
                    Nav::Reload => {
                        let scale = 0.5 + 0.5 * t;
                        icon.opacity(t)
                            .with_transformation(Transformation::scale(size(scale, scale)))
                    }
                }
            })
            .into_any_element()
        };
        button_frame(id)
            .size(px(24.))
            .child(icon)
            .on_click(cx.listener(move |this, _, _, cx| {
                if nav == Nav::Reload && this.loading {
                    this.stop(cx);
                } else {
                    this.nav(nav, cx);
                }
            }))
    }
}

impl BrowserView {
    /// Open the downloads list (⌥⌘L, the toolbar button), taking keyboard
    /// focus from the page so Esc closes it.
    fn open_downloads(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.downloads.read(cx).list.is_empty() {
            return;
        }
        let _ = self.webview.read(cx).raw().focus_parent();
        window.focus(&self.focus);
        self.set_downloads_open(true, cx);
    }

    pub fn set_downloads_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.downloads_open != open {
            self.downloads_open = open;
            if !open {
                self.downloads_closed_at = Some(std::time::Instant::now());
            }
            self.sync_native(cx);
        }
    }

    /// The toolbar's downloads button, once there are any: a bar under the
    /// arrow shows running ones' progress, and it bounces when one starts.
    fn render_downloads_button(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.downloads.read(cx);
        if d.list.is_empty() {
            return None;
        }
        let running: Vec<_> = d
            .list
            .iter()
            .filter(|d| d.state == downloads::State::Running)
            .collect();
        let fraction = (!running.is_empty()).then(|| {
            let (written, total) = running.iter().fold((0u64, 0u64), |(w, t), d| {
                (w + d.written, t + d.total.unwrap_or(0))
            });
            if total > 0 {
                (written as f32 / total as f32).clamp(0.05, 1.)
            } else {
                0.3
            }
        });
        let starts = d.starts;
        let tint = if fraction.is_some() || self.downloads_open {
            pal().accent_bright
        } else {
            pal().muted
        };
        let arrow = icon("download").text_color(rgb(tint)).with_animation(
            ("download-bounce", starts),
            Animation::new(Duration::from_millis(450)),
            |icon, t| {
                let lift = 4. * (PI * t).sin();
                icon.with_transformation(Transformation::translate(point(px(0.), px(-lift))))
            },
        );
        Some(
            button_frame("downloads")
                .size(px(24.))
                .relative()
                .when(self.downloads_open, |b| b.bg(rgba(pal().hover)))
                .child(arrow)
                .children(fraction.map(|f| {
                    div()
                        .absolute()
                        .bottom(px(2.))
                        .left(px(5.))
                        .w(px(14.))
                        .h(px(2.))
                        .rounded(px(1.))
                        .bg(crate::theme::tint(0x26))
                        .child(
                            div()
                                .h_full()
                                .w(relative(f))
                                .rounded(px(1.))
                                .bg(rgb(pal().accent_bright)),
                        )
                }))
                .on_click(cx.listener(|this, _, window, cx| {
                    let just_closed = this
                        .downloads_closed_at
                        .is_some_and(|t| t.elapsed() < Duration::from_millis(300));
                    if !this.downloads_open && !just_closed {
                        this.open_downloads(window, cx);
                    }
                }))
                .into_any_element(),
        )
    }

    /// The downloads list, under the toolbar at the right.
    fn render_downloads(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let list = self.downloads.read(cx).list.clone();
        let rows: Vec<_> = list
            .iter()
            .rev()
            .map(|d| {
                let id = d.id;
                let status = match d.state {
                    downloads::State::Running => match d.total {
                        Some(t) => {
                            format!("{} of {}", downloads::size(d.written), downloads::size(t))
                        }
                        None => format!("{} so far", downloads::size(d.written)),
                    },
                    downloads::State::Done => downloads::size(d.written),
                    downloads::State::Failed => "Failed".into(),
                    downloads::State::Canceled => "Canceled".into(),
                };
                let running = d.state == downloads::State::Running;
                let done = d.state == downloads::State::Done;
                let path = d.path.clone();
                let reveal = path.clone();
                div()
                    .id(("download", id as usize))
                    .mx(px(6.))
                    .px(px(8.))
                    .py(px(6.))
                    .rounded(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .when(done, |r| {
                        r.cursor_pointer().hover(|s| s.bg(rgba(pal().hover)))
                    })
                    .on_click(move |_, _, _| {
                        if done {
                            let _ = std::process::Command::new("open").arg(&path).spawn();
                        }
                    })
                    .child(icon("download").flex_none().text_color(rgb(pal().muted)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(12.5))
                                    .text_color(rgb(pal().text))
                                    .child(d.name()),
                            )
                            .children(d.fraction().filter(|_| running).map(|f| {
                                div().h(px(3.)).rounded(px(2.)).bg(tint(0x1a)).child(
                                    div()
                                        .h_full()
                                        .w(relative(f))
                                        .rounded(px(2.))
                                        .bg(rgb(pal().accent_bright)),
                                )
                            }))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(pal().muted))
                                    .child(status),
                            ),
                    )
                    .when(running, |r| {
                        r.child(icon_button(("download-cancel", id as usize), "x").on_click(
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.downloads.update(cx, |d, cx| d.cancel(id, cx));
                            }),
                        ))
                    })
                    .when(done, |r| {
                        r.child(
                            icon_button(("download-reveal", id as usize), "search").on_click(
                                move |_, _, cx| {
                                    cx.stop_propagation();
                                    let _ = std::process::Command::new("open")
                                        .arg("-R")
                                        .arg(&reveal)
                                        .spawn();
                                },
                            ),
                        )
                    })
            })
            .collect();
        let finished = list.iter().any(|d| d.state != downloads::State::Running);
        div()
            .id("downloads-list")
            .absolute()
            .top(px(4.))
            .right(px(8.))
            .w(px(340.))
            .max_h(px(420.))
            .overflow_y_scroll()
            .py(px(6.))
            .flex()
            .flex_col()
            .gap(px(1.))
            .rounded(px(8.))
            .bg(rgb(pal().menu_bg))
            .border_1()
            .border_color(rgb(pal().menu_border))
            .shadow_lg()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.set_downloads_open(false, cx)))
            .child(
                div()
                    .px(px(14.))
                    .pb(px(4.))
                    .flex()
                    .items_center()
                    .text_size(px(11.))
                    .text_color(rgb(pal().muted))
                    .child(div().flex_1().child("Downloads"))
                    .when(finished, |h| {
                        h.child(
                            div()
                                .id("downloads-clear")
                                .cursor_pointer()
                                .hover(|s| s.text_color(rgb(pal().text)))
                                .child("Clear")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.downloads.update(cx, |d, cx| d.clear(cx));
                                    if this.downloads.read(cx).list.is_empty() {
                                        this.set_downloads_open(false, cx);
                                    }
                                })),
                        )
                    }),
            )
            .children(rows)
    }
}

impl Render for BrowserView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        for img in self.stale_covers.drain(..) {
            let _ = window.drop_image(img);
        }
        let cover = match &self.cover {
            Cover::Shown(img) => Some(img.clone()),
            Cover::Pending { hidden: true, .. } => Some(None),
            _ => None,
        }
        .map(|img| {
            div()
                .absolute()
                .inset_0()
                .bg(rgb(pal().pane_bg))
                .children(img.map(|img| gpui::img(img).size_full()))
        });
        let downloads_button = self.render_downloads_button(cx);
        let downloads = self.downloads_open.then(|| self.render_downloads(cx));
        let address = if let Some(path) = self
            .markdown
            .then(|| markdown::path_of(&self.url))
            .flatten()
        {
            // A Markdown tab shows the file it's on instead of an address bar.
            div()
                .flex_1()
                .min_w_0()
                .h(px(24.))
                .mx_1()
                .px_2()
                .flex()
                .items_center()
                .gap_1p5()
                .text_size(px(12.))
                .text_color(rgb(pal().muted))
                .child(icon("markdown").size(px(13.)).text_color(rgb(pal().muted)))
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .child(crate::workspace::tilde(&path.to_string_lossy())),
                )
                .into_any_element()
        } else {
            div()
                .flex_1()
                .h(px(24.))
                .mx_1()
                .px_2()
                .flex()
                .items_center()
                .rounded(px(6.))
                .bg(rgb(pal().input_bg))
                .text_size(px(12.))
                .key_context(OMNIBAR)
                .on_action(
                    cx.listener(|this, _: &SuggestNext, _, cx| this.move_suggestion(true, cx)),
                )
                .on_action(
                    cx.listener(|this, _: &SuggestPrev, _, cx| this.move_suggestion(false, cx)),
                )
                .on_action(cx.listener(|this, _: &DismissSuggestions, window, cx| {
                    this.dismiss_suggestions(window, cx)
                }))
                .on_action(
                    cx.listener(|this, _: &RemoveSuggestion, _, cx| this.remove_suggestion(cx)),
                )
                .child(
                    Input::new(&self.address)
                        .appearance(false)
                        .text_size(px(12.)),
                )
                .into_any_element()
        };
        let downloads_button = if self.markdown {
            None
        } else {
            downloads_button
        };
        let find_bar = self
            .find
            .shown()
            .map(|input| self.render_find_bar(input, cx));
        div()
            .key_context("Browser")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &Back, _, cx| this.nav(Nav::Back, cx)))
            .on_action(cx.listener(|this, _: &Forward, _, cx| this.nav(Nav::Forward, cx)))
            .on_action(cx.listener(|this, _: &Reload, _, cx| this.nav(Nav::Reload, cx)))
            .on_action(
                cx.listener(|this, _: &ToggleDevTools, _, cx| this.show_inspector(false, cx)),
            )
            .on_action(cx.listener(|this, _: &ShowConsole, _, cx| this.show_inspector(true, cx)))
            .on_action(
                cx.listener(|this, _: &ZoomIn, _, cx| {
                    this.set_zoom(zoom_step(this.zoom, true), cx)
                }),
            )
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| {
                this.set_zoom(zoom_step(this.zoom, false), cx)
            }))
            .on_action(cx.listener(|this, _: &ActualSize, _, cx| this.set_zoom(1.0, cx)))
            .on_action(cx.listener(|this, _: &ToggleDownloads, window, cx| {
                if this.downloads_open {
                    this.set_downloads_open(false, cx);
                } else {
                    this.open_downloads(window, cx);
                }
            }))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                if this.downloads_open && ev.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.set_downloads_open(false, cx);
                    this.focus_page(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Find, window, cx| this.open_find(window, cx)))
            .on_action(cx.listener(|this, _: &FindNext, _, cx| this.find_step(Step::Down, cx)))
            .on_action(cx.listener(|this, _: &FindPrevious, _, cx| this.find_step(Step::Up, cx)))
            .on_action(cx.listener(|this, _: &UseSelectionForFind, _, cx| this.find_selection(cx)))
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(pal().pane_bg))
            .child(
                div()
                    .h(px(34.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .border_b_1()
                    .border_color(rgb(pal().border))
                    .child(self.render_nav(Nav::Back, cx))
                    .child(self.render_nav(Nav::Forward, cx))
                    .child(self.render_nav(Nav::Reload, cx))
                    .child(address)
                    .children(downloads_button),
            )
            .children(find_bar)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(self.webview.clone())
                    .children(cover)
                    .children(self.render_suggestions(cx))
                    .children(downloads),
            )
    }
}

/// Context menus open now: pages under them show as bitmaps, so the menus
/// aren't hidden behind them.
static MENUS_OPEN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub fn menus_open() -> bool {
    MENUS_OPEN.load(std::sync::atomic::Ordering::Relaxed) > 0
}

/// Count a context menu as open until it's dismissed (or dropped). Call it
/// from the menu's builder.
pub fn track_menu(window: &mut Window, cx: &mut Context<gpui_component::menu::PopupMenu>) {
    use std::{cell::Cell, rc::Rc, sync::atomic::Ordering};
    MENUS_OPEN.fetch_add(1, Ordering::Relaxed);
    let done = Rc::new(Cell::new(false));
    let close = {
        let done = done.clone();
        move || {
            if !done.replace(true) {
                MENUS_OPEN.fetch_sub(1, Ordering::Relaxed);
            }
        }
    };
    let on_dismiss = close.clone();
    cx.subscribe_in(
        &cx.entity(),
        window,
        move |_, _, _: &DismissEvent, window, _| {
            on_dismiss();
            window.refresh();
        },
    )
    .detach();
    cx.on_release(move |_, _| close()).detach();
    window.refresh();
}

/// Yields after each left click in the app. Pages are native views, so gpui
/// never sees clicks into them.
pub fn watch_clicks() -> mpsc::UnboundedReceiver<()> {
    let (tx, rx) = mpsc::unbounded();
    let handler = block::ConcreteBlock::new(move |event: *mut Object| -> *mut Object {
        let _ = tx.unbounded_send(());
        event
    })
    .copy();
    // NSEventMaskLeftMouseDown. AppKit copies the block and keeps the monitor.
    let _: *mut Object = unsafe {
        msg_send![objc::class!(NSEvent), addLocalMonitorForEventsMatchingMask: 1u64 << 1 handler: &*handler]
    };
    rx
}

/// An NSImage as PNG bytes.
unsafe fn png_data(image: *mut Object) -> Option<Vec<u8>> {
    if image.is_null() {
        return None;
    }
    unsafe {
        let tiff: *mut Object = msg_send![image, TIFFRepresentation];
        if tiff.is_null() {
            return None;
        }
        let rep: *mut Object = msg_send![objc::class!(NSBitmapImageRep), imageRepWithData: tiff];
        if rep.is_null() {
            return None;
        }
        let props: *mut Object = msg_send![objc::class!(NSDictionary), dictionary];
        // NSBitmapImageFileTypePNG
        let png: *mut Object = msg_send![rep, representationUsingType: 4u64 properties: props];
        if png.is_null() {
            return None;
        }
        let len: usize = msg_send![png, length];
        let bytes: *const u8 = msg_send![png, bytes];
        (!bytes.is_null()).then(|| std::slice::from_raw_parts(bytes, len).to_vec())
    }
}

/// A bare WKWebView's user agent lacks Safari's `Version/… Safari/…` part,
/// and sites like Google then serve their legacy pages. So the pane claims
/// to be the installed Safari.
fn user_agent() -> &'static str {
    static UA: OnceLock<String> = OnceLock::new();
    UA.get_or_init(|| {
        format!(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 \
             (KHTML, like Gecko) Version/{} Safari/605.1.15",
            safari_version(installed_safari_version().as_deref())
        )
    })
}

/// The Safari version to claim: the installed one, but never older than
/// 26.6, as Google Workspace supports only the two latest versions.
fn safari_version(installed: Option<&str>) -> String {
    const FLOOR: [u32; 2] = [26, 6];
    let parsed: Option<Vec<u32>> =
        installed.and_then(|v| v.split('.').map(|n| n.parse().ok()).collect());
    match parsed {
        Some(v) if v.as_slice() >= FLOOR.as_slice() => installed.unwrap().into(),
        _ => format!("{}.{}", FLOOR[0], FLOOR[1]),
    }
}

fn installed_safari_version() -> Option<String> {
    let path = std::ffi::CString::new("/Applications/Safari.app").ok()?;
    let key = std::ffi::CString::new("CFBundleShortVersionString").ok()?;
    unsafe {
        let path: *mut Object =
            msg_send![objc::class!(NSString), stringWithUTF8String: path.as_ptr()];
        let bundle: *mut Object = msg_send![objc::class!(NSBundle), bundleWithPath: path];
        if bundle.is_null() {
            return None;
        }
        let key: *mut Object =
            msg_send![objc::class!(NSString), stringWithUTF8String: key.as_ptr()];
        let version: *mut Object = msg_send![bundle, objectForInfoDictionaryKey: key];
        let is_string: bool =
            !version.is_null() && msg_send![version, isKindOfClass: objc::class!(NSString)];
        if !is_string {
            return None;
        }
        let p: *const std::ffi::c_char = msg_send![version, UTF8String];
        (!p.is_null()).then(|| std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned())
    }
}

const SEARCH: &str = "https://www.google.com/search?q=";

/// Whether `url` is a search `normalize_url` made.
fn is_search(url: &str) -> bool {
    url.starts_with(SEARCH)
}

pub fn normalize_url(text: &str) -> String {
    if text.contains("://") || text.starts_with("about:") {
        text.into()
    } else if text.contains('.') && !text.contains(' ') || text.starts_with("localhost") {
        format!("https://{text}").replace("https://localhost", "http://localhost")
    } else {
        let q: String = text
            .bytes()
            .map(|b| match b {
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {
                    (b as char).to_string()
                }
                b' ' => "+".into(),
                _ => format!("%{b:02X}"),
            })
            .collect();
        format!("{SEARCH}{q}")
    }
}

#[cfg(test)]
mod tests {
    use super::{safari_version, user_agent, zoom_step};

    #[test]
    fn safari_version_keeps_installed_but_not_below_floor() {
        assert_eq!(safari_version(Some("26.6")), "26.6");
        assert_eq!(safari_version(Some("26.6.1")), "26.6.1");
        assert_eq!(safari_version(Some("27.0")), "27.0");
        assert_eq!(safari_version(Some("26")), "26.6");
        assert_eq!(safari_version(Some("18.5")), "26.6");
        assert_eq!(safari_version(Some("bogus")), "26.6");
        assert_eq!(safari_version(None), "26.6");
    }

    #[test]
    fn zoom_steps_through_levels_and_stops_at_the_ends() {
        assert_eq!(zoom_step(1.0, true), 1.15);
        assert_eq!(zoom_step(1.0, false), 0.85);
        assert_eq!(zoom_step(3.0, true), 3.0);
        assert_eq!(zoom_step(0.5, false), 0.5);
        // A zoom between steps (restored from elsewhere) goes to the next one.
        assert_eq!(zoom_step(1.1, true), 1.15);
        assert_eq!(zoom_step(1.1, false), 1.0);
    }

    #[test]
    fn user_agent_looks_like_safari() {
        let ua = user_agent();
        assert!(
            ua.contains(" Version/") && ua.ends_with(" Safari/605.1.15"),
            "{ua}"
        );
    }
}
