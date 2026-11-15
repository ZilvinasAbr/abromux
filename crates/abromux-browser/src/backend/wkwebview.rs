//! macOS pages: a WKWebView through wry and gpui-component, plus the native
//! parts in `abromux_platform` (frames, cookies, hover label, bitmaps,
//! downloads, passkeys, the cursor guard).
#![allow(unexpected_cfgs)] // objc's msg_send!

use super::{Answer, PageBackend, PageEvent, PageOptions, Pixels};
use abromux_platform::{browser_native, cursor, webauthn};
use futures::channel::{mpsc, oneshot};
use gpui::{AnyElement, App, AppContext as _, Entity, IntoElement as _, Window};
use gpui_component::{
    webview::WebView,
    wry::{self, WebViewExtMacOS},
};
use objc::{msg_send, runtime::Object, sel, sel_impl};
use std::sync::{Mutex, OnceLock};

pub struct WkPage {
    webview: Entity<WebView>,
    /// The WKWebView, which `webview` keeps alive.
    wk: *mut Object,
}

impl WkPage {
    pub fn new(
        o: PageOptions,
        events: mpsc::UnboundedSender<PageEvent>,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        let (tx_title, tx_url, tx_ipc) = (events.clone(), events.clone(), events.clone());
        let (tx_dl_start, tx_dl_done) = (events.clone(), events.clone());
        let builder = wry::WebViewBuilder::new();
        // A blank page gets the pane's background rather than white. A URL
        // loads once the WebAuthn bridge is in, so its first page has it.
        let builder = if o.url.is_empty() {
            builder.with_html(format!(
                "<html style=\"background:#{:06x}\"></html>",
                o.blank_bg
            ))
        } else {
            builder
        };
        let mut raw = builder
            .with_user_agent(user_agent())
            .with_devtools(true)
            .with_document_title_changed_handler(move |t| {
                let _ = tx_title.unbounded_send(PageEvent::Title(t));
            })
            .with_on_page_load_handler(move |ev, url| {
                let finished = matches!(ev, wry::PageLoadEvent::Finished);
                let _ = tx_url.unbounded_send(PageEvent::Url { url, finished });
            });
        for script in o.init_scripts {
            raw = raw.with_initialization_script(script);
        }
        let raw = raw
            // WebKit saves it where wry says (a free name in ~/Downloads).
            .with_download_started_handler(move |url, path| {
                let _ = tx_dl_start.unbounded_send(PageEvent::DownloadStarted {
                    url,
                    path: path.clone(),
                });
                true
            })
            .with_download_completed_handler(move |url, _, ok| {
                let _ = tx_dl_done.unbounded_send(PageEvent::DownloadDone { url, ok });
            })
            .with_ipc_handler(move |req| {
                let _ = tx_ipc.unbounded_send(PageEvent::Ipc(req.body().clone()));
            });
        let raw = match o.protocol {
            Some((scheme, serve)) => {
                raw.with_custom_protocol(scheme.into(), move |_, req| serve(&req))
            }
            None => raw,
        };
        let raw = match o.navigation {
            Some(decide) => raw.with_navigation_handler(decide),
            None => raw,
        };
        let raw = raw.build_as_child(window).expect("create webview");
        let wk = {
            let wk = raw.webview();
            &*wk as *const _ as *mut Object
        };
        webauthn::install(wk, o.webauthn_script);
        let (page_tx, mut page_rx) = mpsc::unbounded::<String>();
        browser_native::install(wk, o.frame_script, page_tx);
        // wry's delegate class exists now that a webview does.
        browser_native::downloads_install();
        browser_native::page_shortcuts_install();
        // NSViews don't clip by default since macOS 14; keep the page from
        // drawing past its frame over the focused pane's border just outside.
        let _: () = unsafe { msg_send![wk, setClipsToBounds: true] };
        cursor::install();
        if !o.url.is_empty() {
            let _ = raw.load_url(&o.url);
        }
        let webview = cx.new(|cx| WebView::new(raw, window, cx));
        cx.foreground_executor()
            .spawn(async move {
                use futures::StreamExt as _;
                while let Some(json) = page_rx.next().await {
                    if events
                        .unbounded_send(PageEvent::FrameMessage(json))
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();
        Self { webview, wk }
    }
}

impl PageBackend for WkPage {
    fn element(&self) -> AnyElement {
        self.webview.clone().into_any_element()
    }

    fn set_shown(&self, shown: bool, cx: &mut App) {
        self.webview
            .update(cx, |w, _| if shown { w.show() } else { w.hide() });
    }

    fn load_url(&self, url: &str, cx: &mut App) {
        self.webview.update(cx, |w, _| w.load_url(url));
    }

    fn reload(&self, cx: &App) {
        let _ = self.webview.read(cx).raw().reload();
    }

    fn stop(&self) {
        let _: () = unsafe { msg_send![self.wk, stopLoading] };
    }

    fn is_loading(&self) -> bool {
        unsafe { msg_send![self.wk, isLoading] }
    }

    fn set_zoom(&self, zoom: f64) {
        let _: () = unsafe { msg_send![self.wk, setPageZoom: zoom] };
    }

    fn run(&self, js: &str, cx: &App) {
        let _ = self.webview.read(cx).raw().evaluate_script(js);
    }

    fn eval(&self, js: &str, cx: &App) -> oneshot::Receiver<Answer> {
        let (tx, rx) = oneshot::channel();
        let tx = Mutex::new(Some(tx));
        // On a wry thread; a dropped script leaves the receiver canceled.
        let _ = self
            .webview
            .read(cx)
            .raw()
            .evaluate_script_with_callback(js, move |out| {
                if let Some(tx) = tx.lock().unwrap().take() {
                    let _ = tx.send(Ok(out));
                }
            });
        rx
    }

    fn eval_in_frame(&self, token: &str, js: &str) -> oneshot::Receiver<Answer> {
        browser_native::eval_in_frame(self.wk, token, js)
    }

    fn frame_known(&self, token: &str) -> bool {
        browser_native::frame_known(self.wk, token)
    }

    /// WKWebView's `interactionState` (its back/forward list), base64.
    fn history_state(&self) -> Option<String> {
        unsafe {
            let state: *mut Object = msg_send![self.wk, interactionState];
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

    fn restore_history(&self, state: &str) -> bool {
        let Ok(c) = std::ffi::CString::new(state) else {
            return false;
        };
        unsafe {
            let s: *mut Object =
                msg_send![objc::class!(NSString), stringWithUTF8String: c.as_ptr()];
            let data: *mut Object = msg_send![objc::class!(NSData), alloc];
            let data: *mut Object = msg_send![data, initWithBase64EncodedString: s options: 0usize];
            if data.is_null() {
                return false;
            }
            let _: () = msg_send![self.wk, setInteractionState: data];
            let _: () = msg_send![data, release];
            // WebKit ignores data it can't read; then there's no page.
            let list: *mut Object = msg_send![self.wk, backForwardList];
            let item: *mut Object = msg_send![list, currentItem];
            !item.is_null()
        }
    }

    fn focus(&self, cx: &App) {
        let _ = self.webview.read(cx).raw().focus();
    }

    fn unfocus(&self, cx: &App) {
        let _ = self.webview.read(cx).raw().focus_parent();
    }

    /// The window's first responder is the WKWebView or a view inside it.
    fn has_focus(&self) -> bool {
        unsafe {
            let window: *mut Object = msg_send![self.wk, window];
            if window.is_null() {
                return false;
            }
            let responder: *mut Object = msg_send![window, firstResponder];
            let is_view: bool =
                !responder.is_null() && msg_send![responder, isKindOfClass: objc::class!(NSView)];
            is_view && msg_send![responder, isDescendantOf: self.wk]
        }
    }

    fn bitmap(&self) -> oneshot::Receiver<Option<Pixels>> {
        browser_native::snapshot(self.wk)
    }

    /// WKWebView's own snapshot.
    fn png(&self) -> oneshot::Receiver<Option<Vec<u8>>> {
        let (tx, rx) = oneshot::channel();
        let tx = Mutex::new(Some(tx));
        let handler = block::ConcreteBlock::new(move |image: *mut Object, _error: *mut Object| {
            if let Some(tx) = tx.lock().unwrap().take() {
                let _ = tx.send(unsafe { png_data(image) });
            }
        })
        .copy();
        let nil: *mut Object = std::ptr::null_mut();
        let _: () = unsafe {
            msg_send![self.wk, takeSnapshotWithConfiguration: nil completionHandler: &*handler]
        };
        rx
    }

    fn cookies(&self) -> oneshot::Receiver<Answer> {
        browser_native::cookies(self.wk)
    }

    fn set_cookies(&self, json: &str) -> oneshot::Receiver<Answer> {
        browser_native::set_cookies(self.wk, json)
    }

    fn delete_cookies(&self, json: &str) -> oneshot::Receiver<Answer> {
        browser_native::delete_cookies(self.wk, json)
    }

    fn hover(&self, url: &str) {
        browser_native::hover(self.wk, url);
    }

    /// WebKit's inspector (⌥⌘I), or its console (⌥⌘C).
    fn show_inspector(&self, console: bool) {
        unsafe {
            let inspector: *mut Object = msg_send![self.wk, _inspector];
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
}

/// Yields after each left click in the app.
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
pub fn user_agent() -> &'static str {
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

#[cfg(test)]
mod tests {
    use super::*;

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
