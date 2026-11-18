//! Safe wrappers over browser_native.m: the page script's messages from
//! every frame, scripts run in a chosen frame, cookies, the hover label,
//! page bitmaps and download progress.

use futures::channel::{mpsc, oneshot};
use objc::runtime::Object;
use std::{
    ffi::{CStr, CString, c_char, c_void},
    sync::OnceLock,
};

type MessageFn = extern "C" fn(*mut c_void, *const c_char);
type FreeFn = extern "C" fn(*mut c_void);
type DoneFn = extern "C" fn(*mut c_void, *const c_char, *const c_char);
type ImageFn = extern "C" fn(*mut c_void, *const u8, u32, u32);

unsafe extern "C" {
    fn abx_page_install(
        webview: *mut c_void,
        script: *const c_char,
        ctx: *mut c_void,
        message: MessageFn,
        free: FreeFn,
    );
    fn abx_frame_known(webview: *mut c_void, token: *const c_char) -> bool;
    fn abx_eval_in_frame(
        webview: *mut c_void,
        token: *const c_char,
        script: *const c_char,
        ctx: *mut c_void,
        done: DoneFn,
    );
    fn abx_cookies_get(webview: *mut c_void, ctx: *mut c_void, done: DoneFn);
    fn abx_cookies_set(webview: *mut c_void, json: *const c_char, ctx: *mut c_void, done: DoneFn);
    fn abx_cookies_delete(
        webview: *mut c_void,
        json: *const c_char,
        ctx: *mut c_void,
        done: DoneFn,
    );
    fn abx_hover(webview: *mut c_void, url: *const c_char);
    fn abx_page_view_mark(view: *mut c_void);
    fn abx_snapshot(webview: *mut c_void, ctx: *mut c_void, done: ImageFn);
    fn abx_downloads_install();
    fn abx_page_shortcuts_install();
    fn abx_download_progress(url: *const c_char, written: *mut i64, total: *mut i64) -> bool;
    fn abx_download_cancel(url: *const c_char) -> bool;
    fn abx_download_forget(url: *const c_char);
    fn abx_open_url_hook_install(hook: extern "C" fn(*const c_char) -> bool);
}

/// A script's JSON result, or why it didn't run.
pub type Answer = Result<String, String>;

fn cstr(s: &str) -> CString {
    CString::new(s.replace('\0', "")).expect("no NUL")
}

unsafe fn owned(p: *const c_char) -> Option<String> {
    (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

extern "C" fn on_message(ctx: *mut c_void, json: *const c_char) {
    let tx = unsafe { &*(ctx as *const mpsc::UnboundedSender<String>) };
    if let Some(json) = unsafe { owned(json) } {
        let _ = tx.unbounded_send(json);
    }
}

extern "C" fn free_sender(ctx: *mut c_void) {
    drop(unsafe { Box::from_raw(ctx as *mut mpsc::UnboundedSender<String>) });
}

/// Add the page `script` to every frame of `wk`, before its first
/// navigation. Each message's JSON goes to `tx`.
pub fn install(wk: *mut Object, script: &str, tx: mpsc::UnboundedSender<String>) {
    let script = cstr(script);
    let ctx = Box::into_raw(Box::new(tx)) as *mut c_void;
    unsafe { abx_page_install(wk as _, script.as_ptr(), ctx, on_message, free_sender) };
}

pub fn frame_known(wk: *mut Object, token: &str) -> bool {
    let token = cstr(token);
    unsafe { abx_frame_known(wk as _, token.as_ptr()) }
}

extern "C" fn on_done(ctx: *mut c_void, json: *const c_char, error: *const c_char) {
    let tx = unsafe { Box::from_raw(ctx as *mut oneshot::Sender<Answer>) };
    let out = match unsafe { owned(error) } {
        Some(e) => Err(e),
        None => Ok(unsafe { owned(json) }.unwrap_or_else(|| "null".into())),
    };
    let _ = tx.send(out);
}

fn answer(f: impl FnOnce(*mut c_void)) -> oneshot::Receiver<Answer> {
    let (tx, rx) = oneshot::channel();
    f(Box::into_raw(Box::new(tx)) as *mut c_void);
    rx
}

/// Run `js` in the frame whose page script made `token`.
pub fn eval_in_frame(wk: *mut Object, token: &str, js: &str) -> oneshot::Receiver<Answer> {
    let (token, js) = (cstr(token), cstr(js));
    answer(|ctx| unsafe { abx_eval_in_frame(wk as _, token.as_ptr(), js.as_ptr(), ctx, on_done) })
}

/// Every cookie the webview has, as a JSON array.
pub fn cookies(wk: *mut Object) -> oneshot::Receiver<Answer> {
    answer(|ctx| unsafe { abx_cookies_get(wk as _, ctx, on_done) })
}

/// Set the cookies in a JSON array; answers with how many.
pub fn set_cookies(wk: *mut Object, json: &str) -> oneshot::Receiver<Answer> {
    let json = cstr(json);
    answer(|ctx| unsafe { abx_cookies_set(wk as _, json.as_ptr(), ctx, on_done) })
}

/// Delete the cookies a JSON array of {name, domain, path} names; answers
/// with how many.
pub fn delete_cookies(wk: *mut Object, json: &str) -> oneshot::Receiver<Answer> {
    let json = cstr(json);
    answer(|ctx| unsafe { abx_cookies_delete(wk as _, json.as_ptr(), ctx, on_done) })
}

/// Treat `view` (a page that isn't a WKWebView) as a page for the app's
/// shortcuts, which go to the app first while a page has focus.
pub fn mark_page_view(view: *mut Object) {
    unsafe { abx_page_view_mark(view as _) };
}

/// Show the link under the pointer at the corner of the page view `wk`
/// (any NSView); "" hides it.
pub fn hover(wk: *mut Object, url: &str) {
    let url = cstr(url);
    unsafe { abx_hover(wk as _, url.as_ptr()) };
}

/// A page bitmap: premultiplied BGRA, width, height.
pub type Pixels = (Vec<u8>, u32, u32);

extern "C" fn on_image(ctx: *mut c_void, bgra: *const u8, w: u32, h: u32) {
    let tx = unsafe { Box::from_raw(ctx as *mut oneshot::Sender<Option<Pixels>>) };
    let px = (!bgra.is_null() && w > 0 && h > 0).then(|| {
        let len = w as usize * h as usize * 4;
        (
            unsafe { std::slice::from_raw_parts(bgra, len) }.to_vec(),
            w,
            h,
        )
    });
    let _ = tx.send(px);
}

/// The page as shown. None if WebKit couldn't take it.
pub fn snapshot(wk: *mut Object) -> oneshot::Receiver<Option<Pixels>> {
    let (tx, rx) = oneshot::channel();
    let ctx = Box::into_raw(Box::new(tx)) as *mut c_void;
    unsafe { abx_snapshot(wk as _, ctx, on_image) };
    rx
}

/// Hook wry's navigation delegate for download progress and
/// `Content-Disposition: attachment`. Needs a webview made first (wry
/// registers its class then); later calls do nothing.
pub fn downloads_install() {
    unsafe { abx_downloads_install() };
}

/// Let the app's shortcuts (⌘T, ⌘W, ⌘=, …) work while a page has keyboard
/// focus; keys the app doesn't handle still reach the page. Once is enough.
pub fn page_shortcuts_install() {
    unsafe { abx_page_shortcuts_install() };
}

/// Bytes written and expected (None if unknown) of the running download
/// from `url`; None once it's over.
pub fn download_progress(url: &str) -> Option<(u64, Option<u64>)> {
    let url = cstr(url);
    let (mut written, mut total) = (0i64, -1i64);
    unsafe { abx_download_progress(url.as_ptr(), &mut written, &mut total) }
        .then(|| (written.max(0) as u64, (total >= 0).then_some(total as u64)))
}

pub fn download_cancel(url: &str) -> bool {
    let url = cstr(url);
    unsafe { abx_download_cancel(url.as_ptr()) }
}

pub fn download_forget(url: &str) {
    let url = cstr(url);
    unsafe { abx_download_forget(url.as_ptr()) };
}

/// Asked about each URL gpui is about to open in the system browser.
static OPEN_URL_HOOK: OnceLock<fn(&str) -> bool> = OnceLock::new();

extern "C" fn on_open_url(url: *const c_char) -> bool {
    let (Some(url), Some(hook)) = (unsafe { owned(url) }, OPEN_URL_HOOK.get()) else {
        return false;
    };
    std::panic::catch_unwind(|| hook(&url)).unwrap_or(false)
}

/// Ask `hook` before gpui opens a URL in the system browser; true keeps it
/// from opening there. Once is enough; later hooks are ignored.
pub fn open_url_hook_install(hook: fn(&str) -> bool) {
    let _ = OPEN_URL_HOOK.set(hook);
    unsafe { abx_open_url_hook_install(on_open_url) };
}
