//! There's no built-in browser on Linux, so there are no downloads to track
//! and no page links to catch. Only what code outside the WKWebView backend
//! calls is here.

/// Nothing to install.
pub fn downloads_install() {}

/// Nothing to install.
pub fn page_shortcuts_install() {}

/// No download is ever in progress.
pub fn download_progress(_url: &str) -> Option<(u64, Option<u64>)> {
    None
}

/// No download to cancel.
pub fn download_cancel(_url: &str) -> bool {
    false
}

pub fn download_forget(_url: &str) {}

/// No page opens URLs, so the hook is never called.
pub fn open_url_hook_install(_hook: fn(&str) -> bool) {}
