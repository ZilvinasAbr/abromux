#![allow(unexpected_cfgs)]
//! A browser pane: address bar + native WKWebView (via wry / gpui-component).

use crate::{
    browser_find,
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
use std::{
    f32::consts::PI,
    sync::{Mutex, OnceLock},
    time::Duration,
};

actions!(
    browser,
    [Back, Forward, Reload, ToggleDevTools, ShowConsole]
);

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
}

enum WebMsg {
    Title(String),
    Url(String),
    /// A find script's result, or the page selection for ⌘E.
    Find(String),
    Selection(String),
}

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
    /// The find bar's input, kept after closing so ⌘F brings the last search back.
    find_input: Option<Entity<InputState>>,
    find_open: bool,
    /// Match count and selected index, for the find bar.
    find_count: (usize, Option<usize>),
    /// Presses per toolbar button; each new one replays its animation.
    /// Reload's counts swaps between its reload and stop icons instead.
    nav_presses: [usize; 3],
    /// WKWebView's `isLoading`, while `watch_loading` polls it.
    loading: bool,
    polling: bool,
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
        let (tx_title, tx_url) = (tx.clone(), tx.clone());
        let builder = wry::WebViewBuilder::new();
        // A blank page gets the pane's background rather than white. A URL
        // loads once the WebAuthn bridge is in, so its first page has it.
        let builder = if url.is_empty() {
            builder.with_html(format!("<html style=\"background:#{PANE_BG:06x}\"></html>"))
        } else {
            builder
        };
        let raw = builder
            .with_user_agent(user_agent())
            .with_devtools(true)
            .with_document_title_changed_handler(move |t| {
                let _ = tx_title.unbounded_send(WebMsg::Title(t));
            })
            .with_on_page_load_handler(move |_, url| {
                let _ = tx_url.unbounded_send(WebMsg::Url(url));
            })
            .build_as_child(window)
            .expect("create webview");
        webauthn::install(&raw);
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
                    if !text.is_empty() {
                        this.navigate(&normalize_url(&text), cx);
                        this.focus_page(window, cx);
                    }
                }
                InputEvent::Focus => cx.emit(BrowserEvent::Focused),
                _ => {}
            },
        )
        .detach();

        cx.spawn_in(window, async move |this, cx| {
            while let Some(msg) = rx.next().await {
                let ok = this.update_in(cx, |this, window, cx| {
                    match msg {
                        WebMsg::Title(t) => this.title = t,
                        WebMsg::Url(u) => {
                            let u = if u.starts_with("about:blank") {
                                String::new()
                            } else {
                                u
                            };
                            this.url = u.clone();
                            this.watch_loading(cx);
                            this.address.update(cx, |a, cx| a.set_value(u, window, cx));
                            // A new document has no highlights; search it afresh.
                            this.run_find(cx);
                        }
                        WebMsg::Find(json) => {
                            this.find_count = browser_find::parse_count(&json).unwrap_or_default();
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
                                if let Some(input) = &this.find_input {
                                    input.update(cx, |i, cx| i.set_value(text, window, cx));
                                }
                                this.run_find(cx);
                            }
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

        Self {
            webview,
            address,
            title: String::new(),
            url: url.into(),
            custom_title: None,
            focus: cx.focus_handle(),
            shown: true,
            tx,
            find_input: None,
            find_open: false,
            find_count: (0, None),
            nav_presses: [0; 3],
            loading: false,
            polling: false,
        }
    }

    pub fn navigate(&mut self, url: &str, cx: &mut Context<Self>) {
        self.url = url.into();
        self.webview.update(cx, |w, _| w.load_url(url));
        self.watch_loading(cx);
    }

    pub fn focus_address(this: &Entity<Self>, window: &mut Window, cx: &mut App) {
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
    pub fn set_shown(&mut self, shown: bool, cx: &mut App) {
        if self.shown != shown {
            self.shown = shown;
            self.webview
                .update(cx, |w, _| if shown { w.show() } else { w.hide() });
        }
    }

    pub fn script(&self, js: &str, cx: &App) {
        let _ = self.webview.read(cx).raw().evaluate_script(js);
    }

    pub fn reload(&self, cx: &App) {
        let _ = self.webview.read(cx).raw().reload();
    }

    fn stop(&self, cx: &App) {
        let _: () = unsafe { msg_send![self.wk_webview(cx), stopLoading] };
    }

    fn wk_webview(&self, cx: &App) -> *mut Object {
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
        let input = match &self.find_input {
            Some(input) => input.clone(),
            None => {
                let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find in page"));
                cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, _, cx| match ev {
                    InputEvent::Change => this.run_find(cx),
                    InputEvent::PressEnter { .. } => this.find_step(true, cx),
                    InputEvent::Focus => cx.emit(BrowserEvent::Focused),
                    _ => {}
                })
                .detach();
                self.find_input = Some(input.clone());
                input
            }
        };
        if !self.find_open {
            self.find_open = true;
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
        if !self.find_open {
            return;
        }
        let needle = self
            .find_input
            .as_ref()
            .map(|i| i.read(cx).value().to_string())
            .unwrap_or_default();
        if needle.is_empty() {
            self.find_count = (0, None);
            self.find_script(&browser_find::clear(), cx);
        } else {
            self.find_script(&browser_find::search(&needle), cx);
        }
        cx.notify();
    }

    /// ⌘G / ⇧⌘G (Return / ⇧Return in the bar): the next or previous match.
    fn find_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.find_open {
            self.find_script(&browser_find::step(forward), cx);
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
        self.find_open = false;
        self.find_count = (0, None);
        self.find_script(&browser_find::clear(), cx);
        self.focus_page(window, cx);
        cx.notify();
    }

    fn render_find_bar(&self, input: Entity<InputState>, cx: &mut Context<Self>) -> Stateful<Div> {
        let count = match self.find_count {
            (total, Some(i)) => format!("{}/{total}", i + 1),
            (total, None) => format!("-/{total}"),
        };
        div()
            .id("find-bar")
            .key_context("FindBar")
            .h(px(32.))
            .flex_none()
            .px_2()
            .flex()
            .items_center()
            .gap_1()
            .border_b_1()
            .border_color(rgb(BORDER))
            .bg(rgb(TABBAR_BG))
            .text_size(px(12.))
            .text_color(rgb(TEXT))
            .on_action(
                cx.listener(|this, _: &input::Escape, window, cx| this.close_find(window, cx)),
            )
            .child(icon("search"))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Input::new(&input).appearance(false).text_size(px(12.))),
            )
            .child(div().flex_none().text_color(rgb(MUTED)).child(count))
            .child(
                icon_button("find-up", "chevron-up")
                    .on_click(cx.listener(|this, _, _, cx| this.find_step(false, cx))),
            )
            .child(
                icon_button("find-down", "chevron-down")
                    .on_click(cx.listener(|this, _, _, cx| this.find_step(true, cx))),
            )
            .child(
                icon_button("find-close", "x")
                    .on_click(cx.listener(|this, _, window, cx| this.close_find(window, cx))),
            )
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

    /// `eval`, as a future of the JSON. Canceled if the page drops the script.
    pub fn eval_async(&self, js: &str, cx: &App) -> oneshot::Receiver<String> {
        let (tx, rx) = oneshot::channel();
        let tx = Mutex::new(Some(tx));
        let _ = self.eval(js, cx, move |out| {
            if let Some(tx) = tx.lock().unwrap().take() {
                let _ = tx.send(out);
            }
        });
        rx
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
        let icon = icon(name).text_color(rgb(MUTED));
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

impl Render for BrowserView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let find_bar = self
            .find_input
            .clone()
            .filter(|_| self.find_open)
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
            .on_action(cx.listener(|this, _: &Find, window, cx| this.open_find(window, cx)))
            .on_action(cx.listener(|this, _: &FindNext, _, cx| this.find_step(true, cx)))
            .on_action(cx.listener(|this, _: &FindPrevious, _, cx| this.find_step(false, cx)))
            .on_action(cx.listener(|this, _: &UseSelectionForFind, _, cx| this.find_selection(cx)))
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(PANE_BG))
            .child(
                div()
                    .h(px(34.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(self.render_nav(Nav::Back, cx))
                    .child(self.render_nav(Nav::Forward, cx))
                    .child(self.render_nav(Nav::Reload, cx))
                    .child(
                        div()
                            .flex_1()
                            .h(px(24.))
                            .mx_1()
                            .px_2()
                            .flex()
                            .items_center()
                            .rounded(px(6.))
                            .bg(rgb(INPUT_BG))
                            .text_size(px(12.))
                            .child(
                                Input::new(&self.address)
                                    .appearance(false)
                                    .text_size(px(12.)),
                            ),
                    ),
            )
            .children(find_bar)
            .child(div().flex_1().min_h_0().child(self.webview.clone()))
    }
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
        format!("https://www.google.com/search?q={q}")
    }
}

#[cfg(test)]
mod tests {
    use super::{safari_version, user_agent};

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
    fn user_agent_looks_like_safari() {
        let ua = user_agent();
        assert!(
            ua.contains(" Version/") && ua.ends_with(" Safari/605.1.15"),
            "{ua}"
        );
    }
}
