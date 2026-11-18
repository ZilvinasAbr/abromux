//! macOS pages on Chromium, through CEF (the Chromium Embedded Framework):
//! the `chromium` browser engine.
//!
//! Each page is a CEF browser whose NSView sits in the window above gpui,
//! like a WKWebView. Chromium runs in the app's own run loop: CEF's
//! "external message pump" asks for time, and [`pump`] gives it on the main
//! queue. Its child processes (renderer, GPU, network…) are the helper apps
//! `scripts/bundle-app.sh` puts next to the framework in
//! `Contents/Frameworks`, so the engine works only in a bundled app.
//!
//! Everything else the pane asks of a page goes through the DevTools
//! protocol on the page's own session, so the helpers need no code of ours:
//! scripts (`Runtime.evaluate`), the page and frame scripts
//! (`Page.addScriptToEvaluateOnNewDocument`) and their messages
//! (`Runtime.addBinding`), cookies, screenshots and the back/forward list.
//! Out-of-process (cross-site) iframes are their own DevTools targets, so
//! `browser frame` reaches same-site frames only.
//!
//! WebAuthn is Chromium's own: security keys over USB/NFC and phones over
//! Bluetooth work without Apple's web-browser entitlement; iCloud Keychain
//! passkeys need it, as for WebKit.

use super::{Answer, PageBackend, PageEvent, PageOptions, Pixels};
use abromux_platform::{browser_native, chromium as native, dirs};
use cef::{ImplBrowser as _, ImplBrowserHost as _, ImplFrame as _, *};
use futures::channel::{mpsc, oneshot};
use gpui::{
    AnyElement, App as GpuiApp, Bounds, Element, ElementId, GlobalElementId, Hitbox, IntoElement,
    LayoutId, MouseDownEvent, Pixels as Px, Style, Window,
};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    ffi::c_void,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

// MARK: Engine

thread_local! {
    /// Whether CEF started (Ok) or why it can't (Err). Main thread only.
    static STARTED: RefCell<Option<Result<(), String>>> = const { RefCell::new(None) };
    /// The framework, loaded for as long as the app runs.
    static LIBRARY: RefCell<Option<library_loader::LibraryLoader>> = const { RefCell::new(None) };
}

const FRAMEWORK: &str = "Chromium Embedded Framework.framework";

/// The app bundle's `Contents`, if the app runs from one.
fn contents_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let macos = exe.parent()?;
    (macos.file_name()? == "MacOS").then(|| macos.parent().map(Path::to_path_buf))?
}

/// Whether this build has the engine: the framework and helpers are in the
/// app bundle. Err says why not, for the user.
pub fn available() -> Result<(), String> {
    let contents = contents_dir().ok_or("Chromium works only in the app bundle (AbroMux.app)")?;
    let frameworks = contents.join("Frameworks");
    if !frameworks.join(FRAMEWORK).exists() {
        return Err("this AbroMux.app was built without Chromium".into());
    }
    if !helper_path(&frameworks).exists() {
        return Err("this AbroMux.app has no Chromium helper".into());
    }
    Ok(())
}

/// `Contents/Frameworks/<exe> Helper.app/Contents/MacOS/<exe> Helper`;
/// Chromium finds the (GPU), (Renderer)… variants beside it by name.
fn helper_path(frameworks: &Path) -> PathBuf {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|e| e.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "abromux".into());
    let helper = format!("{exe} Helper");
    frameworks
        .join(format!("{helper}.app"))
        .join("Contents/MacOS")
        .join(helper)
}

/// Start CEF, once. Main thread.
pub fn start() -> Result<(), String> {
    if let Some(done) = STARTED.with_borrow(|s| s.clone()) {
        return done;
    }
    let result = start_now();
    if let Err(e) = &result {
        eprintln!("abromux: Chromium didn't start: {e}");
    }
    STARTED.set(Some(result.clone()));
    result
}

fn start_now() -> Result<(), String> {
    available()?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let loader = library_loader::LibraryLoader::new(&exe, false);
    if !loader.load() {
        return Err("can't load Chromium Embedded Framework".into());
    }
    LIBRARY.set(Some(loader));
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
    native::app_install();

    let root = dirs::support_dir().join("chromium");
    let _ = std::fs::create_dir_all(&root);
    let s = |p: &Path| CefString::from(p.to_string_lossy().as_ref());
    // No browser_subprocess_path: CEF's default, worked out from the app's
    // executable, is the same helper, and only then does Chromium pick its
    // (Renderer), (GPU)… variants, whose entitlements V8 needs.
    let settings = Settings {
        external_message_pump: 1,
        no_sandbox: 0,
        root_cache_path: s(&root),
        // The profile: sign-ins, cookies and site data, kept across runs.
        cache_path: s(&root.join("Default")),
        persist_session_cookies: 1,
        log_file: s(&root.join("chromium.log")),
        log_severity: LogSeverity::WARNING,
        keychain_service_name: CefString::from(
            format!(
                "{} Chromium Safe Storage",
                abromux_platform::channel::app_name()
            )
            .as_str(),
        ),
        ..Default::default()
    };
    let args = args::Args::new();
    let mut app = AbxApp::new();
    if initialize(
        Some(args.as_main_args()),
        Some(&settings),
        Some(&mut app),
        std::ptr::null_mut(),
    ) != 1
    {
        return Err(format!(
            "CEF didn't initialize (see {})",
            root.join("chromium.log").display()
        ));
    }
    schedule(0);
    Ok(())
}

/// Write cookies to disk before the app quits. Nothing if CEF never ran.
pub fn quit() {
    if STARTED.with_borrow(|s| !matches!(s, Some(Ok(())))) {
        return;
    }
    if let Some(cookies) = cookie_manager_get_global_manager(None) {
        cookies.flush_store(None);
    }
}

wrap_app! {
    struct AbxApp;

    impl App {
        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(AbxProcessHandler::new())
        }
    }
}

wrap_browser_process_handler! {
    struct AbxProcessHandler;

    impl BrowserProcessHandler {
        /// Any thread.
        fn on_schedule_message_pump_work(&self, delay_ms: i64) {
            schedule(delay_ms);
        }
    }
}

// MARK: Message pump

/// The latest pump request; a request fires only if none came after it.
static PUMP_GEN: AtomicU64 = AtomicU64::new(0);
/// The longest CEF waits for time: cefclient's external pump runs at least
/// this often, as Chromium's own timers rely on it.
const MAX_DELAY_MS: i64 = 1000 / 30;

thread_local! {
    static IN_PUMP: Cell<bool> = const { Cell::new(false) };
    static PUMP_AGAIN: Cell<bool> = const { Cell::new(false) };
}

#[allow(non_upper_case_globals)]
unsafe extern "C" {
    static _dispatch_main_q: c_void;
    fn dispatch_time(when: u64, delta: i64) -> u64;
    fn dispatch_after_f(
        when: u64,
        queue: *const c_void,
        context: *mut c_void,
        work: extern "C" fn(*mut c_void),
    );
}

/// Give CEF time on the main queue in `delay_ms`, instead of any earlier
/// request. Any thread.
fn schedule(delay_ms: i64) {
    let generation = PUMP_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    let delay = delay_ms.clamp(0, MAX_DELAY_MS) * 1_000_000;
    unsafe {
        dispatch_after_f(
            dispatch_time(0, delay),
            &raw const _dispatch_main_q,
            generation as *mut c_void,
            fire,
        );
    }
}

extern "C" fn fire(generation: *mut c_void) {
    if generation as u64 == PUMP_GEN.load(Ordering::SeqCst) {
        pump();
    }
}

fn pump() {
    if IN_PUMP.get() {
        // CEF ran a nested loop that asked again: once it's out.
        PUMP_AGAIN.set(true);
        return;
    }
    let before = PUMP_GEN.load(Ordering::SeqCst);
    IN_PUMP.set(true);
    do_message_loop_work();
    IN_PUMP.set(false);
    if PUMP_AGAIN.replace(false) {
        schedule(0);
    } else if PUMP_GEN.load(Ordering::SeqCst) == before {
        schedule(MAX_DELAY_MS);
    }
}

// MARK: Page state

/// Each DevTools method's answer: its `result`, or the error message.
type Reply = Box<dyn FnOnce(Result<Value, String>)>;

/// What a page's handlers share. CEF calls them on its UI thread, which is
/// the main thread here, like the pane.
struct Inner {
    events: mpsc::UnboundedSender<PageEvent>,
    browser: RefCell<Option<Browser>>,
    /// The NSView CEF made for the page.
    view: Cell<*mut c_void>,
    shown: Cell<bool>,
    loading: Cell<bool>,
    next_id: Cell<i32>,
    replies: RefCell<HashMap<i32, Reply>>,
    /// The URL to load once the page scripts are in, so its first document
    /// has them.
    first_url: RefCell<Option<String>>,
    frames: RefCell<Frames>,
    /// The back/forward list as last read, for `history_state`.
    history: RefCell<Option<String>>,
    navigation: Option<super::Navigation>,
}

/// Which JavaScript context is which frame, and the frame scripts' tokens.
#[derive(Default)]
struct Frames {
    main: Option<String>,
    /// Frame id → URL.
    urls: HashMap<String, String>,
    /// Default execution context id → frame id.
    contexts: HashMap<i64, String>,
    /// A frame script's token → its execution context.
    tokens: HashMap<String, i64>,
}

type Shared = Arc<Inner>;

impl Inner {
    fn send(&self, ev: PageEvent) {
        let _ = self.events.unbounded_send(ev);
    }

    fn host(&self) -> Option<BrowserHost> {
        self.browser.borrow().as_ref()?.host()
    }

    fn is_ours(&self, browser: Option<&mut Browser>) -> bool {
        let (Some(b), Some(mine)) = (browser, self.browser.borrow().clone()) else {
            return false;
        };
        b.identifier() == mine.identifier()
    }

    /// Call DevTools `method`; `reply` gets its result.
    fn call(&self, method: &str, params: Value, reply: Reply) {
        let id = self.next_id.get() + 1;
        self.next_id.set(id);
        let msg = json!({ "id": id, "method": method, "params": params }).to_string();
        let sent = self
            .host()
            .is_some_and(|h| h.send_dev_tools_message(Some(msg.as_bytes())) == 1);
        if sent {
            self.replies.borrow_mut().insert(id, reply);
        } else {
            reply(Err("the page is gone".into()));
        }
    }

    /// Call `method` for its side effect.
    fn tell(&self, method: &str, params: Value) {
        self.call(method, params, Box::new(|_| {}));
    }

    /// Call `method`; the receiver gets `f` of its result.
    fn answer(
        &self,
        method: &str,
        params: Value,
        f: impl FnOnce(Value) -> Answer + 'static,
    ) -> oneshot::Receiver<Answer> {
        let (tx, rx) = oneshot::channel();
        self.call(
            method,
            params,
            Box::new(move |r| {
                let _ = tx.send(r.and_then(f));
            }),
        );
        rx
    }

    /// `Runtime.evaluate` `js` (in `context`, else the top frame); answers
    /// with the result's JSON.
    fn evaluate(&self, js: &str, context: Option<i64>) -> oneshot::Receiver<Answer> {
        let mut params = json!({
            "expression": js,
            "returnByValue": true,
            "awaitPromise": true,
            "userGesture": true,
        });
        if let Some(id) = context {
            params["contextId"] = json!(id);
        }
        let (tx, rx) = oneshot::channel();
        self.call(
            "Runtime.evaluate",
            params,
            Box::new(move |r| {
                let _ = tx.send(r.and_then(|v| script_result(&v)));
            }),
        );
        rx
    }

    fn method_result(&self, id: i32, ok: bool, result: &[u8]) {
        let Some(reply) = self.replies.borrow_mut().remove(&id) else {
            return;
        };
        let value: Value = serde_json::from_slice(result).unwrap_or(Value::Null);
        reply(if ok {
            Ok(value)
        } else {
            Err(value["message"]
                .as_str()
                .unwrap_or("the page refused")
                .to_string())
        });
    }

    fn event(&self, method: &str, params: &[u8]) {
        let Ok(p) = serde_json::from_slice::<Value>(params) else {
            return;
        };
        match method {
            "Runtime.bindingCalled" => self.binding(&p),
            "Runtime.executionContextCreated" => {
                let c = &p["context"];
                let aux = &c["auxData"];
                if aux["isDefault"].as_bool() == Some(true)
                    && let (Some(id), Some(frame)) = (c["id"].as_i64(), aux["frameId"].as_str())
                {
                    self.frames
                        .borrow_mut()
                        .contexts
                        .insert(id, frame.to_string());
                }
            }
            "Runtime.executionContextDestroyed" => {
                if let Some(id) = p["executionContextId"].as_i64() {
                    let mut f = self.frames.borrow_mut();
                    f.contexts.remove(&id);
                    f.tokens.retain(|_, c| *c != id);
                }
            }
            "Runtime.executionContextsCleared" => {
                let mut f = self.frames.borrow_mut();
                f.contexts.clear();
                f.tokens.clear();
            }
            "Page.frameNavigated" => {
                let frame = &p["frame"];
                if let (Some(id), Some(url)) = (frame["id"].as_str(), frame["url"].as_str()) {
                    let mut f = self.frames.borrow_mut();
                    if frame["parentId"].is_null() {
                        f.main = Some(id.to_string());
                    }
                    f.urls.insert(id.to_string(), url.to_string());
                }
            }
            "Page.frameDetached" => {
                if let Some(id) = p["frameId"].as_str() {
                    self.frames.borrow_mut().urls.remove(id);
                }
            }
            _ => {}
        }
    }

    /// A page or frame script called one of the page's bindings.
    fn binding(&self, p: &Value) {
        let (Some(name), Some(payload)) = (p["name"].as_str(), p["payload"].as_str()) else {
            return;
        };
        let context = p["executionContextId"].as_i64().unwrap_or(0);
        let (main, frame_url) = {
            let f = self.frames.borrow();
            let frame = f.contexts.get(&context);
            let main = frame.is_some_and(|id| f.main.as_ref() == Some(id));
            (main, frame.and_then(|id| f.urls.get(id)).cloned())
        };
        match name {
            IPC_BINDING if main => self.send(PageEvent::Ipc(payload.to_string())),
            PAGE_BINDING => {
                let Ok(Value::Object(mut body)) = serde_json::from_str::<Value>(payload) else {
                    return;
                };
                let kind = body.get("kind").and_then(Value::as_str).unwrap_or("");
                if let Some(token) = body.get("token").and_then(Value::as_str)
                    && (kind == "hello" || kind == "answer")
                {
                    let mut f = self.frames.borrow_mut();
                    // A new document in the main frame: its old frames are gone.
                    if main && kind == "hello" {
                        f.tokens.clear();
                    }
                    f.tokens.insert(token.to_string(), context);
                }
                body.insert("main".into(), json!(main));
                if let Some(url) = frame_url {
                    body.insert("frameUrl".into(), json!(url));
                }
                self.send(PageEvent::FrameMessage(Value::Object(body).to_string()));
            }
            _ => {}
        }
    }

    /// Read the back/forward list into `history`.
    fn refresh_history(self: &Shared) {
        let me = Arc::downgrade(self);
        self.call(
            "Page.getNavigationHistory",
            json!({}),
            Box::new(move |r| {
                if let (Some(me), Ok(v)) = (me.upgrade(), r) {
                    let entries: Vec<Value> = v["entries"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .map(|e| json!({ "url": e["url"], "title": e["title"] }))
                                .collect()
                        })
                        .unwrap_or_default();
                    let state = json!({ "index": v["currentIndex"], "entries": entries });
                    *me.history.borrow_mut() = Some(format!("{HISTORY_PREFIX}{state}"));
                }
            }),
        );
    }
}

/// A `Runtime.evaluate` result as the JSON of its value, or the exception.
fn script_result(v: &Value) -> Answer {
    if let Some(ex) = v.get("exceptionDetails") {
        let text = ex["exception"]["description"]
            .as_str()
            .or_else(|| ex["text"].as_str())
            .unwrap_or("the script threw");
        return Err(text.to_string());
    }
    let r = &v["result"];
    Ok(match r.get("value") {
        Some(value) => value.to_string(),
        None if r["type"] == "undefined" => "null".into(),
        None => r
            .get("unserializableValue")
            .and_then(Value::as_str)
            .map_or_else(|| "null".into(), |s| json!(s).to_string()),
    })
}

/// Prompt, Cancel and timeouts for WebAuthn requests (see the script).
const WEBAUTHN_SCRIPT: &str = include_str!("chromium_webauthn.js");

const PAGE_BINDING: &str = "__abromuxPageBinding";
const IPC_BINDING: &str = "__abromuxIpcBinding";
/// Marks this backend's `history_state`, so a WebKit one isn't misread.
const HISTORY_PREFIX: &str = "chromium:";

/// The scripts as Chromium runs them: WebKit's message handlers and wry's
/// `window.ipc` become the page's bindings. In a block, so the names don't
/// leak to the page.
fn shim(script: &str, top_only: bool) -> String {
    let body = script.replace("window.webkit", "__abromuxWebkit");
    let guard = if top_only {
        "if (window !== window.top) return;"
    } else {
        ""
    };
    format!(
        "(() => {{ {guard}
  const bind = name => (m) => {{ try {{ globalThis[name](typeof m === 'string' ? m : JSON.stringify(m)); }} catch (_) {{}} }};
  const __abromuxWebkit = {{ messageHandlers: {{ abromuxPage: {{ postMessage: bind('{PAGE_BINDING}') }} }} }};
  if (!window.ipc) Object.defineProperty(window, 'ipc', {{ value: {{ postMessage: bind('{IPC_BINDING}') }} }});
{body}
}})();"
    )
}

// MARK: Handlers

wrap_client! {
    struct AbxClient {
        inner: Shared,
    }

    impl Client {
        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(AbxDisplay::new(self.inner.clone()))
        }
        fn load_handler(&self) -> Option<LoadHandler> {
            Some(AbxLoad::new(self.inner.clone()))
        }
        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(AbxLifeSpan::new(self.inner.clone()))
        }
        fn request_handler(&self) -> Option<RequestHandler> {
            Some(AbxRequest::new(self.inner.clone()))
        }
        fn download_handler(&self) -> Option<DownloadHandler> {
            Some(AbxDownload::new(self.inner.clone()))
        }
    }
}

wrap_display_handler! {
    struct AbxDisplay {
        inner: Shared,
    }

    impl DisplayHandler {
        fn on_title_change(&self, browser: Option<&mut Browser>, title: Option<&CefString>) {
            if self.inner.is_ours(browser) {
                self.inner.send(PageEvent::Title(title.map(|t| t.to_string()).unwrap_or_default()));
            }
        }

        fn on_address_change(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            _url: Option<&CefString>,
        ) {
            if self.inner.is_ours(browser) && frame.is_some_and(|f| f.is_main() == 1) {
                self.inner.refresh_history();
            }
        }
    }
}

wrap_load_handler! {
    struct AbxLoad {
        inner: Shared,
    }

    impl LoadHandler {
        fn on_loading_state_change(
            &self,
            browser: Option<&mut Browser>,
            is_loading: i32,
            _can_go_back: i32,
            _can_go_forward: i32,
        ) {
            if self.inner.is_ours(browser) {
                self.inner.loading.set(is_loading != 0);
            }
        }

        fn on_load_start(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            _transition_type: TransitionType,
        ) {
            if let Some(url) = main_frame_url(&self.inner, browser, frame) {
                self.inner.send(PageEvent::Url { url, finished: false });
            }
        }

        fn on_load_end(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            _http_status_code: i32,
        ) {
            if let Some(url) = main_frame_url(&self.inner, browser, frame) {
                self.inner.send(PageEvent::Url { url, finished: true });
                self.inner.refresh_history();
            }
        }
    }
}

/// The URL of `frame` if it's the main frame of this page.
fn main_frame_url(
    inner: &Inner,
    browser: Option<&mut Browser>,
    frame: Option<&mut Frame>,
) -> Option<String> {
    if !inner.is_ours(browser) {
        return None;
    }
    let frame = frame.filter(|f| f.is_main() == 1)?;
    Some(CefString::from(&frame.url()).to_string())
}

wrap_life_span_handler! {
    struct AbxLifeSpan {
        inner: Shared,
    }

    impl LifeSpanHandler {
        /// A link with a target or `window.open`: a tab beside this one,
        /// like WebKit's panes; only scripted popups (sign-in windows that
        /// talk to their opener) open as Chromium windows.
        fn on_before_popup(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _popup_id: i32,
            target_url: Option<&CefString>,
            _target_frame_name: Option<&CefString>,
            target_disposition: WindowOpenDisposition,
            _user_gesture: i32,
            _popup_features: Option<&PopupFeatures>,
            _window_info: Option<&mut WindowInfo>,
            client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            _no_javascript_access: Option<&mut i32>,
        ) -> i32 {
            let url = target_url.map(|u| u.to_string()).unwrap_or_default();
            if target_disposition == WindowOpenDisposition::NEW_POPUP {
                // Its own window, without this page's handlers.
                if let Some(client) = client {
                    *client = None;
                }
                return 0;
            }
            let focus = target_disposition != WindowOpenDisposition::NEW_BACKGROUND_TAB;
            self.inner
                .send(PageEvent::Ipc(json!({ "openTab": url, "focus": focus }).to_string()));
            1
        }

        fn on_before_close(&self, browser: Option<&mut Browser>) {
            if self.inner.is_ours(browser) {
                self.inner.replies.borrow_mut().clear();
            }
        }
    }
}

wrap_request_handler! {
    struct AbxRequest {
        inner: Shared,
    }

    impl RequestHandler {
        fn on_before_browse(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            request: Option<&mut Request>,
            _user_gesture: i32,
            _is_redirect: i32,
        ) -> i32 {
            let Some(decide) = &self.inner.navigation else {
                return 0;
            };
            if !self.inner.is_ours(browser) || !frame.is_some_and(|f| f.is_main() == 1) {
                return 0;
            }
            let url = request.map(|r| CefString::from(&r.url()).to_string()).unwrap_or_default();
            i32::from(!decide(url))
        }
    }
}

// MARK: Downloads

/// A running download: its progress, and how to cancel it.
struct Download {
    written: u64,
    total: Option<u64>,
    cancel: Option<DownloadItemCallback>,
}

thread_local! {
    /// Chromium's downloads by URL, for the downloads list.
    static DOWNLOADS: RefCell<HashMap<String, Download>> = RefCell::new(HashMap::new());
}

/// A running Chromium download's bytes so far and total, if it's one.
pub fn download_progress(url: &str) -> Option<(u64, Option<u64>)> {
    DOWNLOADS.with_borrow(|d| d.get(url).map(|d| (d.written, d.total)))
}

/// Cancel a Chromium download; false if it's not one.
pub fn download_cancel(url: &str) -> bool {
    DOWNLOADS.with_borrow_mut(|d| match d.get_mut(url).and_then(|d| d.cancel.take()) {
        Some(cancel) => {
            cancel.cancel();
            true
        }
        None => false,
    })
}

pub fn download_forget(url: &str) {
    DOWNLOADS.with_borrow_mut(|d| d.remove(url));
}

/// A name in ~/Downloads no file has yet: `name`, else `name (2).ext`…
fn free_download_path(name: &str) -> PathBuf {
    let dir = dirs::home().join("Downloads");
    let name = Path::new(name)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "download".into());
    let first = dir.join(&name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.clone(), String::new()),
    };
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

wrap_download_handler! {
    struct AbxDownload {
        inner: Shared,
    }

    impl DownloadHandler {
        fn can_download(
            &self,
            _browser: Option<&mut Browser>,
            _url: Option<&CefString>,
            _request_method: Option<&CefString>,
        ) -> i32 {
            1
        }

        fn on_before_download(
            &self,
            _browser: Option<&mut Browser>,
            item: Option<&mut DownloadItem>,
            suggested_name: Option<&CefString>,
            callback: Option<&mut BeforeDownloadCallback>,
        ) -> i32 {
            let (Some(item), Some(callback)) = (item, callback) else {
                return 0;
            };
            let url = CefString::from(&item.url()).to_string();
            let name = suggested_name.map(|n| n.to_string()).unwrap_or_default();
            let path = free_download_path(&name);
            DOWNLOADS.with_borrow_mut(|d| {
                d.insert(url.clone(), Download { written: 0, total: None, cancel: None })
            });
            callback.cont(Some(&CefString::from(path.to_string_lossy().as_ref())), 0);
            self.inner.send(PageEvent::DownloadStarted { url, path });
            1
        }

        fn on_download_updated(
            &self,
            _browser: Option<&mut Browser>,
            item: Option<&mut DownloadItem>,
            callback: Option<&mut DownloadItemCallback>,
        ) {
            let Some(item) = item else { return };
            let url = CefString::from(&item.url()).to_string();
            let done = item.is_complete() == 1;
            let failed = item.is_canceled() == 1 || item.is_interrupted() == 1;
            let known = DOWNLOADS.with_borrow_mut(|d| {
                let Some(d) = d.get_mut(&url) else { return false };
                d.written = item.received_bytes().max(0) as u64;
                d.total = u64::try_from(item.total_bytes()).ok().filter(|t| *t > 0);
                d.cancel = callback.map(|c| c.clone());
                true
            });
            if known && (done || failed) {
                DOWNLOADS.with_borrow_mut(|d| {
                    if let Some(d) = d.get_mut(&url) {
                        d.cancel = None;
                    }
                });
                self.inner.send(PageEvent::DownloadDone { url, ok: done });
            }
        }
    }
}

// MARK: DevTools

wrap_dev_tools_message_observer! {
    struct AbxDevTools {
        inner: Shared,
    }

    impl DevToolsMessageObserver {
        fn on_dev_tools_method_result(
            &self,
            _browser: Option<&mut Browser>,
            message_id: i32,
            success: i32,
            result: Option<&[u8]>,
        ) {
            self.inner.method_result(message_id, success == 1, result.unwrap_or_default());
        }

        fn on_dev_tools_event(
            &self,
            _browser: Option<&mut Browser>,
            method: Option<&CefString>,
            params: Option<&[u8]>,
        ) {
            let method = method.map(|m| m.to_string()).unwrap_or_default();
            self.inner.event(&method, params.unwrap_or_default());
        }
    }
}

// MARK: The page

pub struct CefPage {
    inner: Shared,
    /// Keeps the DevTools observer on.
    _devtools: Option<Registration>,
}

impl CefPage {
    /// A page in `window`, which [`can_make`] said yes to. If Chromium
    /// fails to make it after all, it stays blank (and says why on stderr).
    pub fn new(
        o: PageOptions,
        events: mpsc::UnboundedSender<PageEvent>,
        window: &mut Window,
    ) -> Self {
        let parent = parent_view(window).unwrap_or(std::ptr::null_mut());
        let inner: Shared = Arc::new(Inner {
            events,
            browser: RefCell::new(None),
            view: Cell::new(std::ptr::null_mut()),
            shown: Cell::new(true),
            loading: Cell::new(false),
            next_id: Cell::new(0),
            replies: RefCell::new(HashMap::new()),
            first_url: RefCell::new((!o.url.is_empty()).then(|| o.url.clone())),
            frames: RefCell::new(Frames::default()),
            history: RefCell::new(None),
            navigation: o.navigation,
        });
        let window_info = WindowInfo {
            parent_view: parent,
            bounds: Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
            // Chrome style can't be hosted in another app's view on macOS.
            runtime_style: RuntimeStyle::ALLOY,
            ..Default::default()
        };
        // Pages without a background of their own are white, as in Chrome
        // (this color shows through them). Only a blank tab gets the pane's.
        let settings = BrowserSettings {
            background_color: 0xffff_ffff,
            ..Default::default()
        };
        let mut client = AbxClient::new(inner.clone());
        let browser = browser_host_create_browser_sync(
            Some(&window_info),
            Some(&mut client),
            Some(&CefString::from("about:blank")),
            Some(&settings),
            None,
            None,
        );
        let Some((browser, host)) = browser.and_then(|b| b.host().map(|h| (b, h))) else {
            eprintln!("abromux: Chromium didn't make a page");
            return Self {
                inner,
                _devtools: None,
            };
        };
        let view = host.window_handle() as *mut c_void;
        inner.view.set(view);
        if !view.is_null() {
            browser_native::mark_page_view(view as _);
        }
        *inner.browser.borrow_mut() = Some(browser);
        let mut observer = AbxDevTools::new(inner.clone());
        let devtools = host.add_dev_tools_message_observer(Some(&mut observer));
        browser_native::page_shortcuts_install();

        // The page and frame scripts and their bindings, then the first URL.
        inner.tell("Runtime.enable", json!({}));
        inner.tell("Page.enable", json!({}));
        for name in [PAGE_BINDING, IPC_BINDING] {
            inner.tell("Runtime.addBinding", json!({ "name": name }));
        }
        let mut scripts: Vec<String> = o.init_scripts.iter().map(|s| shim(s, true)).collect();
        scripts.push(shim(o.frame_script, false));
        // Chromium's own WebAuthn, minus what its missing dialogs would do.
        scripts.push(WEBAUTHN_SCRIPT.to_string());
        // A blank tab (about:blank, already there) gets the pane's
        // background rather than white, as for WebKit.
        if o.url.is_empty() {
            let _ = inner.evaluate(
                &format!(
                    "document.documentElement.style.background = '#{:06x}'",
                    o.blank_bg
                ),
                None,
            );
        }
        let last = scripts.len() - 1;
        for (i, source) in scripts.into_iter().enumerate() {
            let me = Arc::downgrade(&inner);
            inner.call(
                "Page.addScriptToEvaluateOnNewDocument",
                json!({ "source": source }),
                Box::new(move |_| {
                    if i == last
                        && let Some(me) = me.upgrade()
                        && let Some(url) = me.first_url.borrow_mut().take()
                        && let Some(frame) =
                            me.browser.borrow().as_ref().and_then(|b| b.main_frame())
                    {
                        frame.load_url(Some(&CefString::from(url.as_str())));
                    }
                }),
            );
        }
        Self {
            inner,
            _devtools: devtools,
        }
    }

    fn browser(&self) -> Option<Browser> {
        self.inner.browser.borrow().clone()
    }

    fn view(&self) -> *mut c_void {
        self.inner.view.get()
    }

    /// The page as shown, as PNG, to `done`.
    fn screenshot(&self, done: impl FnOnce(Option<Vec<u8>>) + 'static) {
        self.inner.call(
            "Page.captureScreenshot",
            json!({ "format": "png" }),
            Box::new(move |r| {
                use base64::Engine as _;
                done(r.ok().and_then(|v| {
                    base64::engine::general_purpose::STANDARD
                        .decode(v["data"].as_str()?)
                        .ok()
                }))
            }),
        );
    }
}

/// Whether a page can be made in `window`: Chromium started, and the window
/// has a view to hold it.
pub fn can_make(window: &Window) -> bool {
    start().is_ok() && parent_view(window).is_some()
}

/// The window's gpui NSView, which pages go into.
fn parent_view(window: &Window) -> Option<*mut c_void> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::AppKit(h) => Some(h.ns_view.as_ptr()),
        _ => None,
    }
}

impl Drop for CefPage {
    fn drop(&mut self) {
        let view = self.view();
        if !view.is_null() {
            native::unfocus(view);
            native::place(view, 0., 0., 0., 0., false);
        }
        if let Some(host) = self.inner.host() {
            host.close_browser(1);
        }
        if !view.is_null() {
            native::remove(view);
        }
    }
}

impl PageBackend for CefPage {
    fn element(&self) -> AnyElement {
        PageElement {
            inner: self.inner.clone(),
        }
        .into_any_element()
    }

    fn set_shown(&self, shown: bool, _cx: &mut GpuiApp) {
        self.inner.shown.set(shown);
        let view = self.view();
        if !shown && !view.is_null() {
            native::unfocus(view);
            native::place(view, 0., 0., 0., 0., false);
        }
        if let Some(host) = self.inner.host() {
            host.was_hidden(i32::from(!shown));
        }
    }

    fn load_url(&self, url: &str, _cx: &mut GpuiApp) {
        // A URL asked for before the scripts are in replaces the first one.
        if self.inner.first_url.borrow().is_some() {
            *self.inner.first_url.borrow_mut() = Some(url.to_string());
            return;
        }
        if let Some(frame) = self.browser().and_then(|b| b.main_frame()) {
            frame.load_url(Some(&CefString::from(url)));
        }
    }

    fn reload(&self, _cx: &GpuiApp) {
        if let Some(b) = self.browser() {
            b.reload();
        }
    }

    fn stop(&self) {
        if let Some(b) = self.browser() {
            b.stop_load();
        }
    }

    fn is_loading(&self) -> bool {
        self.inner.loading.get()
    }

    /// Chromium's zoom levels are steps of 20%: 1.2^level.
    fn set_zoom(&self, zoom: f64) {
        if let Some(host) = self.inner.host() {
            host.set_zoom_level(zoom.max(0.01).ln() / 1.2f64.ln());
        }
    }

    fn run(&self, js: &str, _cx: &GpuiApp) {
        let _ = self.inner.evaluate(js, None);
    }

    fn eval(&self, js: &str, _cx: &GpuiApp) -> oneshot::Receiver<Answer> {
        self.inner.evaluate(js, None)
    }

    fn eval_in_frame(&self, token: &str, js: &str) -> oneshot::Receiver<Answer> {
        let context = self.inner.frames.borrow().tokens.get(token).copied();
        match context {
            Some(id) => self.inner.evaluate(js, Some(id)),
            None => {
                let (tx, rx) = oneshot::channel();
                let _ = tx.send(Err(
                    "that frame is gone (run `browser frame main`, then pick it again)".into(),
                ));
                rx
            }
        }
    }

    fn frame_known(&self, token: &str) -> bool {
        self.inner.frames.borrow().tokens.contains_key(token)
    }

    fn history_state(&self) -> Option<String> {
        self.inner.history.borrow().clone()
    }

    /// Chromium can't be handed a back/forward list, so only its current
    /// page comes back.
    fn restore_history(&self, state: &str) -> bool {
        let Some(v) = state
            .strip_prefix(HISTORY_PREFIX)
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
        else {
            return false;
        };
        let index = v["index"].as_u64().unwrap_or(0) as usize;
        let Some(url) = v["entries"][index]["url"]
            .as_str()
            .filter(|u| !u.is_empty())
        else {
            return false;
        };
        if self.inner.first_url.borrow().is_some() {
            *self.inner.first_url.borrow_mut() = Some(url.to_string());
        } else if let Some(frame) = self.browser().and_then(|b| b.main_frame()) {
            frame.load_url(Some(&CefString::from(url)));
        }
        true
    }

    fn focus(&self, _cx: &GpuiApp) {
        if let Some(host) = self.inner.host() {
            host.set_focus(1);
        }
    }

    fn unfocus(&self, _cx: &GpuiApp) {
        if let Some(host) = self.inner.host() {
            host.set_focus(0);
        }
        let view = self.view();
        if !view.is_null() {
            native::unfocus(view);
        }
    }

    fn has_focus(&self) -> bool {
        let view = self.view();
        !view.is_null() && native::has_focus(view)
    }

    fn bitmap(&self) -> oneshot::Receiver<Option<Pixels>> {
        let (tx, rx) = oneshot::channel();
        self.screenshot(move |png| {
            let _ = tx.send(png.and_then(|png| bgra(&png)));
        });
        rx
    }

    fn png(&self) -> oneshot::Receiver<Option<Vec<u8>>> {
        let (tx, rx) = oneshot::channel();
        self.screenshot(move |png| {
            let _ = tx.send(png);
        });
        rx
    }

    fn cookies(&self) -> oneshot::Receiver<Answer> {
        self.inner.answer("Storage.getCookies", json!({}), |v| {
            let list: Vec<Value> = v["cookies"]
                .as_array()
                .map(|a| a.iter().map(cookie_json).collect())
                .unwrap_or_default();
            Ok(Value::Array(list).to_string())
        })
    }

    fn set_cookies(&self, json: &str) -> oneshot::Receiver<Answer> {
        let cookies = match cookie_params(json) {
            Ok(c) => c,
            Err(e) => return answered(Err(e)),
        };
        let n = cookies.len();
        self.inner.answer(
            "Network.setCookies",
            json!({ "cookies": cookies }),
            move |_| Ok(n.to_string()),
        )
    }

    fn delete_cookies(&self, json: &str) -> oneshot::Receiver<Answer> {
        let Ok(Value::Array(list)) = serde_json::from_str::<Value>(json) else {
            return answered(Err("expected an array of {name, domain, path}".into()));
        };
        let mut n = 0;
        for c in &list {
            let Some(name) = c["name"].as_str() else {
                continue;
            };
            let mut params = json!({ "name": name });
            for key in ["domain", "path"] {
                if let Some(v) = c[key].as_str() {
                    params[key] = json!(v);
                }
            }
            self.inner.tell("Network.deleteCookies", params);
            n += 1;
        }
        answered(Ok(n.to_string()))
    }

    fn hover(&self, url: &str) {
        let view = self.view();
        if !view.is_null() {
            browser_native::hover(view as _, url);
        }
    }

    /// Chromium's DevTools, in a window of its own.
    fn show_inspector(&self, _console: bool) {
        if let Some(host) = self.inner.host() {
            host.show_dev_tools(
                Some(&WindowInfo::default()),
                None,
                Some(&BrowserSettings::default()),
                None,
            );
        }
    }
}

/// A receiver that already has `answer`.
fn answered(answer: Answer) -> oneshot::Receiver<Answer> {
    let (tx, rx) = oneshot::channel();
    let _ = tx.send(answer);
    rx
}

/// A cookie as the pane's JSON: {name, value, domain, path, expires,
/// httpOnly, secure, sameSite}, like WebKit's.
fn cookie_json(c: &Value) -> Value {
    let expires = c["expires"]
        .as_f64()
        .filter(|e| *e > 0.)
        .map_or(-1., f64::floor);
    json!({
        "name": c["name"],
        "value": c["value"],
        "domain": c["domain"],
        "path": c["path"].as_str().unwrap_or("/"),
        "expires": expires,
        "httpOnly": c["httpOnly"].as_bool().unwrap_or(false),
        "secure": c["secure"].as_bool().unwrap_or(false),
        "sameSite": c["sameSite"].as_str().unwrap_or("None"),
    })
}

/// The pane's cookie JSON as `Network.setCookies` params.
fn cookie_params(json: &str) -> Result<Vec<Value>, String> {
    let Ok(Value::Array(list)) = serde_json::from_str::<Value>(json) else {
        return Err("expected an array of cookies".into());
    };
    list.iter()
        .map(|c| {
            let (Some(name), Some(domain)) = (c["name"].as_str(), c["domain"].as_str()) else {
                return Err("each cookie needs a name and a domain".to_string());
            };
            let mut p = json!({
                "name": name,
                "value": c["value"].as_str().unwrap_or(""),
                "domain": domain,
                "path": c["path"].as_str().unwrap_or("/"),
                "secure": c["secure"].as_bool().unwrap_or(false),
                "httpOnly": c["httpOnly"].as_bool().unwrap_or(false),
            });
            if let Some(e) = c["expires"].as_f64().filter(|e| *e > 0.) {
                p["expires"] = json!(e);
            }
            match c["sameSite"]
                .as_str()
                .map(str::to_ascii_lowercase)
                .as_deref()
            {
                Some("strict") => p["sameSite"] = json!("Strict"),
                Some("lax") => p["sameSite"] = json!("Lax"),
                Some("none") => p["sameSite"] = json!("None"),
                _ => {}
            }
            Ok(p)
        })
        .collect()
}

/// A PNG as premultiplied BGRA.
fn bgra(png: &[u8]) -> Option<Pixels> {
    let img = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .ok()?
        .to_rgba8();
    let (w, h) = img.dimensions();
    let mut px = img.into_raw();
    for p in px.chunks_exact_mut(4) {
        let a = p[3] as u32;
        let (r, g, b) = (p[0] as u32, p[1] as u32, p[2] as u32);
        p[0] = (b * a / 255) as u8;
        p[1] = (g * a / 255) as u8;
        p[2] = (r * a / 255) as u8;
    }
    Some((px, w, h))
}

// MARK: The element

/// Where the page goes in the pane: each frame puts its NSView there.
struct PageElement {
    inner: Shared,
}

impl IntoElement for PageElement {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for PageElement {
    type RequestLayoutState = ();
    type PrepaintState = Option<Hitbox>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut GpuiApp,
    ) -> (LayoutId, ()) {
        let style = Style {
            flex_grow: 0.,
            flex_shrink: 1.,
            size: gpui::Size::full(),
            ..Default::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Px>,
        _: &mut (),
        window: &mut Window,
        _: &mut GpuiApp,
    ) -> Option<Hitbox> {
        let view = self.inner.view.get();
        if !self.inner.shown.get() || view.is_null() {
            return None;
        }
        let f = |p: Px| f64::from(f32::from(p));
        native::place(
            view,
            f(bounds.origin.x),
            f(bounds.origin.y),
            f(bounds.size.width),
            f(bounds.size.height),
            true,
        );
        Some(window.insert_hitbox(bounds, gpui::HitboxBehavior::Normal))
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Px>,
        _: &mut (),
        hitbox: &mut Option<Hitbox>,
        window: &mut Window,
        _: &mut GpuiApp,
    ) {
        let bounds = hitbox.as_ref().map_or(bounds, |h| h.bounds);
        let inner = self.inner.clone();
        // A click outside the page takes its keyboard focus, as for WebKit.
        window.on_mouse_event(move |e: &MouseDownEvent, _, _, _| {
            if !bounds.contains(&e.position) {
                let view = inner.view.get();
                if !view.is_null() && native::has_focus(view) {
                    if let Some(host) = inner.host() {
                        host.set_focus(0);
                    }
                    native::unfocus(view);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_results_are_json() {
        let v = json!({ "result": { "type": "number", "value": 3 } });
        assert_eq!(script_result(&v).unwrap(), "3");
        let v = json!({ "result": { "type": "undefined" } });
        assert_eq!(script_result(&v).unwrap(), "null");
        let v = json!({ "result": { "type": "object", "value": { "a": [1] } } });
        assert_eq!(script_result(&v).unwrap(), r#"{"a":[1]}"#);
        let v = json!({
            "result": { "type": "object" },
            "exceptionDetails": { "text": "Uncaught", "exception": { "description": "Error: no" } }
        });
        assert_eq!(script_result(&v).unwrap_err(), "Error: no");
    }

    #[test]
    fn shim_routes_webkit_handlers_to_bindings() {
        let s = shim(
            "const h = window.webkit && window.webkit.messageHandlers.abromuxPage;",
            true,
        );
        assert!(!s.contains("window.webkit"));
        assert!(s.contains(PAGE_BINDING) && s.contains(IPC_BINDING));
        assert!(s.contains("window !== window.top"));
        assert!(!shim("x", false).contains("window !== window.top"));
    }

    #[test]
    fn cookies_round_trip_through_devtools_shapes() {
        let c = cookie_json(&json!({
            "name": "a", "value": "1", "domain": ".x.com", "path": "/",
            "expires": 1700000000.5, "httpOnly": true, "secure": true, "sameSite": "Lax"
        }));
        assert_eq!(c["expires"], json!(1700000000.0));
        assert_eq!(c["sameSite"], "Lax");
        let session =
            cookie_json(&json!({ "name": "s", "value": "", "domain": "x", "expires": -1 }));
        assert_eq!(session["expires"], json!(-1.0));
        assert_eq!(session["sameSite"], "None");

        let p = cookie_params(r#"[{"name":"a","domain":"x.com","sameSite":"strict","expires":5}]"#)
            .unwrap();
        assert_eq!(p[0]["sameSite"], "Strict");
        assert_eq!(p[0]["path"], "/");
        assert!(cookie_params(r#"[{"name":"a"}]"#).is_err());
    }

    #[test]
    fn history_state_is_marked() {
        assert!(HISTORY_PREFIX.ends_with(':'));
    }
}
