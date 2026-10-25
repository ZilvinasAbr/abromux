#![allow(unexpected_cfgs)]
//! A browser pane: address bar + native WKWebView (via wry / gpui-component).

use crate::{
    browser_find,
    terminal::{Find, FindNext, FindPrevious, UseSelectionForFind},
    theme::*,
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
use std::sync::Mutex;

actions!(
    browser,
    [Back, Forward, Reload, ToggleDevTools, ShowConsole]
);

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
        // A blank page gets the pane's background rather than white.
        let builder = if url.is_empty() {
            builder.with_html(format!("<html style=\"background:#{PANE_BG:06x}\"></html>"))
        } else {
            builder.with_url(url)
        };
        let raw = builder
            .with_devtools(true)
            .with_document_title_changed_handler(move |t| {
                let _ = tx_title.unbounded_send(WebMsg::Title(t));
            })
            .with_on_page_load_handler(move |_, url| {
                let _ = tx_url.unbounded_send(WebMsg::Url(url));
            })
            .build_as_child(window)
            .expect("create webview");
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
        }
    }

    pub fn navigate(&mut self, url: &str, cx: &mut Context<Self>) {
        self.url = url.into();
        self.webview.update(cx, |w, _| w.load_url(url));
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
}

impl Render for BrowserView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let nav = |id: &'static str, icon: &'static str| icon_button(id, icon).size(px(24.));
        let find_bar = self
            .find_input
            .clone()
            .filter(|_| self.find_open)
            .map(|input| self.render_find_bar(input, cx));
        div()
            .key_context("Browser")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &Back, _, cx| this.script("history.back()", cx)))
            .on_action(cx.listener(|this, _: &Forward, _, cx| this.script("history.forward()", cx)))
            .on_action(cx.listener(|this, _: &Reload, _, cx| this.reload(cx)))
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
                    .child(
                        nav("back", "chevron-left").on_click(
                            cx.listener(|this, _, _, cx| this.script("history.back()", cx)),
                        ),
                    )
                    .child(nav("fwd", "chevron-right").on_click(
                        cx.listener(|this, _, _, cx| this.script("history.forward()", cx)),
                    ))
                    .child(
                        nav("reload", "rotate-cw")
                            .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                    )
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
