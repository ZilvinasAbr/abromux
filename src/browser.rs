#![allow(unexpected_cfgs)]
//! A browser pane: address bar + native WKWebView (via wry / gpui-component).

use crate::theme::*;
use futures::{StreamExt, channel::mpsc};
use gpui::{prelude::*, *};
use gpui_component::{
    input::{Input, InputEvent, InputState},
    webview::WebView,
    wry::{self, WebViewExtMacOS},
};
use objc::{msg_send, runtime::Object, sel, sel_impl};

pub enum BrowserEvent {
    TitleChanged,
    Focused,
}

enum WebMsg {
    Title(String),
    Url(String),
}

pub struct BrowserView {
    webview: Entity<WebView>,
    address: Entity<InputState>,
    pub title: String,
    pub url: String,
    focus: FocusHandle,
    shown: bool,
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
        let (tx_title, tx_url) = (tx.clone(), tx);
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

        let address = cx.new(|cx| InputState::new(window, cx).placeholder("Search or enter URL").default_value(url.to_string()));
        cx.subscribe_in(&address, window, |this, input, ev: &InputEvent, window, cx| match ev {
            InputEvent::PressEnter { .. } => {
                let text = input.read(cx).value().trim().to_string();
                if !text.is_empty() {
                    this.navigate(&normalize_url(&text), cx);
                    this.focus_page(window, cx);
                }
            }
            InputEvent::Focus => cx.emit(BrowserEvent::Focused),
            _ => {}
        })
        .detach();

        cx.spawn_in(window, async move |this, cx| {
            while let Some(msg) = rx.next().await {
                let ok = this.update_in(cx, |this, window, cx| {
                    match msg {
                        WebMsg::Title(t) => this.title = t,
                        WebMsg::Url(u) => {
                            let u = if u.starts_with("about:blank") { String::new() } else { u };
                            this.url = u.clone();
                            this.address.update(cx, |a, cx| a.set_value(u, window, cx));
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

        Self { webview, address, title: String::new(), url: url.into(), focus: cx.focus_handle(), shown: true }
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
            let is_view: bool = !responder.is_null() && msg_send![responder, isKindOfClass: objc::class!(NSView)];
            is_view && msg_send![responder, isDescendantOf: webview]
        }
    }

    /// Native views float above gpui content, so hidden panes must be hidden
    /// explicitly.
    pub fn set_shown(&mut self, shown: bool, cx: &mut App) {
        if self.shown != shown {
            self.shown = shown;
            self.webview.update(cx, |w, _| if shown { w.show() } else { w.hide() });
        }
    }

    pub fn script(&self, js: &str, cx: &App) {
        let _ = self.webview.read(cx).raw().evaluate_script(js);
    }

    pub fn reload(&self, cx: &App) {
        let _ = self.webview.read(cx).raw().reload();
    }

    /// Run `js` and hand its JSON-encoded result to `done` (on a wry thread).
    pub fn eval(&self, js: &str, cx: &App, done: impl Fn(String) + Send + 'static) -> anyhow::Result<()> {
        Ok(self.webview.read(cx).raw().evaluate_script_with_callback(js, done)?)
    }
}

impl Render for BrowserView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let nav = |id: &'static str, icon: &'static str| icon_button(id, icon).size(px(24.));
        div()
            .track_focus(&self.focus)
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
                    .child(nav("back", "chevron-left").on_click(cx.listener(|this, _, _, cx| this.script("history.back()", cx))))
                    .child(nav("fwd", "chevron-right").on_click(cx.listener(|this, _, _, cx| this.script("history.forward()", cx))))
                    .child(nav("reload", "rotate-cw").on_click(cx.listener(|this, _, _, cx| this.reload(cx))))
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
                            .child(Input::new(&self.address).appearance(false).text_size(px(12.))),
                    ),
            )
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
    let _: *mut Object = unsafe { msg_send![objc::class!(NSEvent), addLocalMonitorForEventsMatchingMask: 1u64 << 1 handler: &*handler] };
    rx
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
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
                b' ' => "+".into(),
                _ => format!("%{b:02X}"),
            })
            .collect();
        format!("https://www.google.com/search?q={q}")
    }
}
