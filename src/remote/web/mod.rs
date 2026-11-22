//! The phone app's files, built into the binary: a plain web app (no build
//! step) with xterm.js 5.5.0 (MIT, `vendor/xterm-LICENSE`) drawing terminals.
//! Claude tabs show in it as the Mac's own chat page (`claude_chat/`), in a
//! frame, with `chat-shim.js` standing in for the web view's IPC.

use crate::claude_chat;
use std::sync::OnceLock;

/// Scripts and styles from the page itself only; the WebSocket to it.
/// Images in Claude's replies may come from the web, as in the Mac's tab.
pub const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
                       img-src 'self' data: https:; connect-src 'self' wss:; worker-src 'self'; \
                       manifest-src 'self'; frame-src 'self'; frame-ancestors 'self'; \
                       base-uri 'none'; form-action 'none'; object-src 'none'";

/// The Claude tab's page, with the phone's shim before its script and the
/// phone's styles after its own.
fn chat_html() -> &'static [u8] {
    static HTML: OnceLock<String> = OnceLock::new();
    HTML.get_or_init(|| {
        claude_chat::PAGE_HTML
            .replace(
                "width=device-width, initial-scale=1",
                "width=device-width, initial-scale=1, viewport-fit=cover",
            )
            .replace(" autofocus>", ">")
            .replace(
                "</head>",
                "<link rel=\"stylesheet\" href=\"/chat-phone.css\">\n\
                 <script src=\"/chat-shim.js\"></script>\n</head>",
            )
    })
    .as_bytes()
}

const HTML: &str = "text/html; charset=utf-8";
const JS: &str = "text/javascript; charset=utf-8";
const CSS: &str = "text/css; charset=utf-8";
const PNG: &str = "image/png";

/// A file by its path: its content type and bytes.
pub fn file(path: &str) -> Option<(&'static str, &'static [u8])> {
    let nightly = crate::channel::nightly();
    let icon = |release: &'static [u8], night: &'static [u8]| if nightly { night } else { release };
    Some(match path {
        "/" | "/index.html" => (HTML, include_bytes!("index.html")),
        "/app.js" => (JS, include_bytes!("app.js")),
        "/app.css" => (CSS, include_bytes!("app.css")),
        "/sw.js" => (JS, include_bytes!("sw.js")),
        "/chat.html" => (HTML, chat_html()),
        "/chat-shim.js" => (JS, include_bytes!("chat-shim.js")),
        "/chat-phone.css" => (CSS, include_bytes!("chat-phone.css")),
        "/__chat/page.js" => (JS, claude_chat::PAGE_JS.as_bytes()),
        "/__chat/page.css" => (CSS, claude_chat::PAGE_CSS.as_bytes()),
        "/__chat/hl.js" => (JS, claude_chat::HIGHLIGHT_JS.as_bytes()),
        "/__chat/logo.svg" => ("image/svg+xml", claude_chat::LOGO_SVG.as_bytes()),
        "/vendor/xterm.js" => (JS, include_bytes!("vendor/xterm.js")),
        "/vendor/xterm.css" => (CSS, include_bytes!("vendor/xterm.css")),
        "/manifest.webmanifest" => (
            "application/manifest+json",
            if nightly {
                MANIFEST_NIGHTLY.as_bytes()
            } else {
                MANIFEST.as_bytes()
            },
        ),
        "/icon-180.png" => (
            PNG,
            icon(
                include_bytes!("abromux-180.png"),
                include_bytes!("abromux-nightly-180.png"),
            ),
        ),
        "/icon-192.png" => (
            PNG,
            icon(
                include_bytes!("abromux-192.png"),
                include_bytes!("abromux-nightly-192.png"),
            ),
        ),
        "/icon-512.png" => (
            PNG,
            icon(
                include_bytes!("abromux-512.png"),
                include_bytes!("abromux-nightly-512.png"),
            ),
        ),
        _ => return mac_icon(path),
    })
}

/// The Mac app's own icons (`assets/icons/`), which the phone paints as
/// masks in its colors.
fn mac_icon(path: &str) -> Option<(&'static str, &'static [u8])> {
    macro_rules! icons {
        ($($name:literal),*) => {
            match path.strip_prefix("/icons/")?.strip_suffix(".svg")? {
                $($name => include_bytes!(concat!("../../../assets/icons/", $name, ".svg")).as_slice(),)*
                _ => return None,
            }
        };
    }
    let svg = icons!(
        "bell",
        "bot",
        "check",
        "chevron-down",
        "chevron-left",
        "chevron-right",
        "claude",
        "codex",
        "folder",
        "git-branch",
        "gitlab",
        "globe",
        "keyboard",
        "markdown",
        "panel-left",
        "plus",
        "search",
        "terminal",
        "x"
    );
    Some(("image/svg+xml", svg))
}

const MANIFEST: &str = r##"{
  "name": "abromux",
  "short_name": "abromux",
  "start_url": "/",
  "scope": "/",
  "display": "standalone",
  "background_color": "#1f1e1d",
  "theme_color": "#1f1e1d",
  "icons": [
    { "src": "/icon-192.png", "sizes": "192x192", "type": "image/png" },
    { "src": "/icon-512.png", "sizes": "512x512", "type": "image/png" }
  ]
}"##;

const MANIFEST_NIGHTLY: &str = r##"{
  "name": "abromux Nightly",
  "short_name": "abromux N",
  "start_url": "/",
  "scope": "/",
  "display": "standalone",
  "background_color": "#1f1e1d",
  "theme_color": "#1f1e1d",
  "icons": [
    { "src": "/icon-192.png", "sizes": "192x192", "type": "image/png" },
    { "src": "/icon-512.png", "sizes": "512x512", "type": "image/png" }
  ]
}"##;

#[cfg(test)]
mod tests {
    #[test]
    fn serves_the_app() {
        for p in [
            "/",
            "/app.js",
            "/sw.js",
            "/vendor/xterm.js",
            "/manifest.webmanifest",
            "/icon-192.png",
            "/chat.html",
            "/__chat/page.js",
            "/__chat/logo.svg",
            "/icons/terminal.svg",
        ] {
            assert!(super::file(p).is_some_and(|(_, b)| !b.is_empty()), "{p}");
        }
        assert!(super::file("/../Cargo.toml").is_none());
        let chat = std::str::from_utf8(super::file("/chat.html").unwrap().1).unwrap();
        assert!(chat.contains("/chat-shim.js") && chat.contains("/__chat/page.js"));
        assert!(chat.find("/chat-shim.js") < chat.find("/__chat/page.js"));
        assert!(!chat.contains("autofocus"));
    }
}
