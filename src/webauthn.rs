//! Passkeys and security keys in browser panes. The page script
//! (`webauthn.js`) hands WebAuthn requests to a native bridge
//! (`abromux_platform::webauthn`) that runs them with AuthenticationServices.
//! That needs the `com.apple.developer.web-browser.public-key-credential`
//! entitlement; a build without it leaves WebAuthn to WebKit.

use gpui_component::wry::{self, WebViewExtMacOS};

const SCRIPT: &str = include_str!("webauthn.js");

/// Add the bridge to a new webview, before its first navigation.
pub fn install(webview: &wry::WebView) {
    let wk = webview.webview();
    abromux_platform::webauthn::install(&*wk as *const _ as *mut _, SCRIPT);
}
