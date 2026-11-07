//! Markdown tabs: a browser tab showing a local Markdown file, rendered to
//! HTML here, that follows the file as it changes (cmux's `markdown`).
//!
//! The page is served from `abromux-md://localhost/<absolute path>`, a scheme
//! only Markdown tabs register, so relative links and images in the file
//! resolve to its neighbours on disk and the browser's own back/forward, find,
//! zoom and automation all work on it. Web pages in other tabs can't reach
//! the scheme, and a Markdown tab only ever shows local files: links to the
//! web open as browser tabs (see `BrowserView`'s navigation handler). Every
//! response carries a policy without scripts, so HTML in a file can't read
//! other files.

use gpui_component::wry::http::{Request, Response};
use std::{
    borrow::Cow,
    collections::HashMap,
    fmt::Write as _,
    path::{Path, PathBuf},
};

pub const SCHEME: &str = "abromux-md";
const PREFIX: &str = "abromux-md://localhost";

/// No scripts or connections of the file's own; images and media from the
/// web (badges) still load.
const CSP: &str = "default-src 'none'; img-src abromux-md: data: https: http:; \
    media-src abromux-md: https: http:; style-src 'unsafe-inline' abromux-md:; \
    font-src abromux-md: data: https:; frame-src abromux-md:; script-src 'none'; \
    object-src 'none'; form-action 'none'; base-uri 'none'";

/// Files read as Markdown.
pub fn is_markdown(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "md" | "markdown" | "mdown" | "mkd" | "mkdn" | "mdx"
        )
    })
}

pub fn is_markdown_url(url: &str) -> bool {
    url.starts_with(SCHEME) && url[SCHEME.len()..].starts_with(':')
}

/// The tab URL for a file (absolute, or made so against the current dir).
pub fn url_for(path: &Path) -> String {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut out = String::from(PREFIX);
    for b in path.to_string_lossy().bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

/// The file a tab URL shows (its path, without query or fragment).
pub fn path_of(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix(SCHEME)?.strip_prefix("://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let end = rest.find(['?', '#']).unwrap_or(rest.len());
    path_from_encoded(&rest[..end])
}

fn path_from_encoded(path: &str) -> Option<PathBuf> {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(h) = path.get(i + 1..i + 3)
            && let Ok(b) = u8::from_str_radix(h, 16)
        {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let path = PathBuf::from(String::from_utf8(out).ok()?);
    path.is_absolute().then_some(path)
}

/// The file's name, for the tab when the page hasn't loaded yet.
pub fn title_of(url: &str) -> Option<String> {
    let path = path_of(url)?;
    Some(
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned()),
    )
}

/// The scheme's answer to WebKit's request.
pub fn respond(req: &Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    let path = path_from_encoded(req.uri().path());
    let (status, mime, body) = match path {
        None => (
            400,
            "text/html; charset=utf-8",
            page("Bad address", &message("That isn't a file's address.")).into(),
        ),
        Some(p) => serve(&p),
    };
    Response::builder()
        .status(status)
        .header("Content-Type", mime)
        .header("Content-Security-Policy", CSP)
        .header("Cache-Control", "no-store")
        .body(Cow::Owned(body.into_bytes()))
        .unwrap_or_else(|_| Response::new(Cow::Borrowed(&[][..])))
}

/// What a response's body holds.
enum Body {
    Text(String),
    Bytes(Vec<u8>),
}

impl Body {
    fn into_bytes(self) -> Vec<u8> {
        match self {
            Body::Text(s) => s.into_bytes(),
            Body::Bytes(b) => b,
        }
    }
}

impl From<String> for Body {
    fn from(s: String) -> Self {
        Body::Text(s)
    }
}

fn serve(path: &Path) -> (u16, &'static str, Body) {
    let name = path.to_string_lossy().into_owned();
    if path.is_dir() {
        return (
            200,
            "text/html; charset=utf-8",
            page(&name, &listing(path)).into(),
        );
    }
    if is_markdown(path) {
        return match std::fs::read_to_string(path) {
            Ok(text) => (
                200,
                "text/html; charset=utf-8",
                page(&file_name(path), &render(&text)).into(),
            ),
            Err(e) => (
                404,
                "text/html; charset=utf-8",
                page(&file_name(path), &missing(path, &e)).into(),
            ),
        };
    }
    match std::fs::read(path) {
        Ok(bytes) => match mime(path) {
            Some(m) => (200, m, Body::Bytes(bytes)),
            // Text shows as itself; anything else isn't the viewer's to open
            // (and WebKit would download it).
            None if !bytes[..bytes.len().min(8192)].contains(&0) => {
                (200, "text/plain; charset=utf-8", Body::Bytes(bytes))
            }
            None => (
                415,
                "text/html; charset=utf-8",
                page(
                    &file_name(path),
                    &message(&format!(
                        "{} isn't something the Markdown viewer shows.",
                        esc(&name)
                    )),
                )
                .into(),
            ),
        },
        Err(e) => (
            404,
            "text/html; charset=utf-8",
            page(&file_name(path), &missing(path, &e)).into(),
        ),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn mime(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "bmp" => "image/bmp",
        "pdf" => "application/pdf",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "css" => "text/css",
        "html" | "htm" => "text/html; charset=utf-8",
        _ => return None,
    })
}

/// The HTML of the file's body, for the page and for a live reload.
pub fn body_of(path: &Path) -> String {
    match std::fs::read_to_string(path) {
        Ok(text) => render(&text),
        Err(e) => missing(path, &e),
    }
}

/// A script that swaps the page's content for the file's as it is now,
/// keeping the scroll position.
pub fn reload_script(path: &Path) -> String {
    let body = serde_json::to_string(&body_of(path)).unwrap_or_else(|_| "\"\"".into());
    format!(
        "(() => {{ const a = document.querySelector('article.markdown-body'); \
         if (a) a.innerHTML = {body}; }})()"
    )
}

fn missing(path: &Path, e: &std::io::Error) -> String {
    let why = match e.kind() {
        std::io::ErrorKind::NotFound => "isn't there (any more)".to_string(),
        _ => format!("couldn't be read: {e}"),
    };
    message(&format!("{} {why}.", esc(&path.to_string_lossy())))
}

fn message(text: &str) -> String {
    format!("<p class=\"abromux-message\">{text}</p>")
}

fn listing(dir: &Path) -> String {
    let mut entries: Vec<(bool, String, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| {
            let p = e.path();
            (p.is_dir(), file_name(&p), p)
        })
        .filter(|(_, n, _)| !n.starts_with('.'))
        .collect();
    entries.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
    });
    let mut out = format!("<h1>{}</h1>\n<ul>\n", esc(&dir.to_string_lossy()));
    if let Some(parent) = dir.parent() {
        let _ = writeln!(out, "<li><a href=\"{}\">..</a></li>", esc(&url_for(parent)));
    }
    for (is_dir, name, p) in entries {
        let slash = if is_dir { "/" } else { "" };
        let _ = writeln!(
            out,
            "<li><a href=\"{}\">{}{slash}</a></li>",
            esc(&url_for(&p)),
            esc(&name)
        );
    }
    out + "</ul>"
}

/// GitHub-flavoured Markdown to HTML, front matter left out, headings given
/// GitHub's ids so `#section` links work.
pub fn render(text: &str) -> String {
    let mut opts = markdown::Options::gfm();
    opts.parse.constructs.frontmatter = true;
    // READMEs use HTML (`<p align="center">`, `<details>`, `<img>`); the
    // page's policy keeps scripts in it from running.
    opts.compile.allow_dangerous_html = true;
    // Kept so `file://` links can become the viewer's (`local_links`); a
    // `javascript:` one can't run under the policy, and the tab follows
    // only its own scheme, the web's and `mailto:`.
    opts.compile.allow_dangerous_protocol = true;
    let html = markdown::to_html_with_options(text, &opts)
        .unwrap_or_else(|_| format!("<pre>{}</pre>", esc(text)));
    heading_ids(&local_links(&html))
}

/// `file://` links and images as the viewer's own URLs: WebKit won't load
/// those from this page.
fn local_links(html: &str) -> String {
    let mut out = html.to_string();
    for attr in ["href", "src"] {
        for q in ['"', '\''] {
            for from in ["file://localhost/", "file:///"] {
                out = out.replace(
                    &format!("{attr}={q}{from}"),
                    &format!("{attr}={q}{PREFIX}/"),
                );
            }
        }
    }
    out
}

/// `<h2>` → `<h2 id="slug">`, with GitHub's slugs (`-1`, `-2` for repeats).
fn heading_ids(html: &str) -> String {
    let mut out = String::with_capacity(html.len() + 256);
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut rest = html;
    while let Some(i) = rest.find("<h") {
        let after = &rest[i + 2..];
        let level = after.as_bytes().first().copied();
        if !matches!(level, Some(b'1'..=b'6')) || after.as_bytes().get(1) != Some(&b'>') {
            out.push_str(&rest[..i + 2]);
            rest = after;
            continue;
        }
        let level = level.unwrap() as char;
        let close = format!("</h{level}>");
        let inner_start = i + 4;
        let Some(len) = rest[inner_start..].find(&close) else {
            break;
        };
        let inner = &rest[inner_start..inner_start + len];
        let base = slug(&strip_tags(inner));
        let n = seen.entry(base.clone()).or_insert(0);
        let id = if *n == 0 {
            base.clone()
        } else {
            format!("{base}-{n}")
        };
        *n += 1;
        out.push_str(&rest[..i]);
        let _ = write!(out, "<h{level} id=\"{}\">{inner}{close}", esc(&id));
        rest = &rest[inner_start + len + close.len()..];
    }
    out.push_str(rest);
    out
}

fn strip_tags(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
}

/// GitHub's heading anchor: lower case, punctuation dropped, spaces to `-`.
fn slug(text: &str) -> String {
    text.trim()
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            '-' | '_' => Some(c),
            c if c.is_alphanumeric() => Some(c),
            _ => None,
        })
        .collect()
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <meta http-equiv=\"Content-Security-Policy\" content=\"{CSP}\">\
         <meta name=\"color-scheme\" content=\"dark\">\
         <title>{}</title><style>{STYLE}</style></head>\
         <body><article class=\"markdown-body\">{body}</article></body></html>",
        esc(title)
    )
}

/// GitHub's Markdown look, in the app's dark colours.
const STYLE: &str = r#"
:root { color-scheme: dark; --bg: #1a1918; --text: #faf9f5; --muted: #9c9a92;
  --border: #3a3935; --code-bg: #262624; --accent: #d97757; --link: #6cb6ff; }
html { background: var(--bg); }
body { margin: 0; color: var(--text); font: 15px/1.6 -apple-system, BlinkMacSystemFont, "Segoe UI", Helvetica, Arial, sans-serif; -webkit-font-smoothing: antialiased; }
.markdown-body { max-width: 860px; margin: 0 auto; padding: 32px 40px 64px; word-wrap: break-word; }
@media (max-width: 600px) { .markdown-body { padding: 20px 16px 48px; } }
.markdown-body > :first-child { margin-top: 0 !important; }
h1, h2, h3, h4, h5, h6 { margin: 24px 0 16px; font-weight: 600; line-height: 1.25; }
h1 { font-size: 2em; padding-bottom: .3em; border-bottom: 1px solid var(--border); }
h2 { font-size: 1.5em; padding-bottom: .3em; border-bottom: 1px solid var(--border); }
h3 { font-size: 1.25em; } h4 { font-size: 1em; } h5 { font-size: .875em; } h6 { font-size: .85em; color: var(--muted); }
p, blockquote, ul, ol, dl, table, pre, details { margin: 0 0 16px; }
a { color: var(--link); text-decoration: none; } a:hover { text-decoration: underline; }
strong { font-weight: 600; }
hr { height: 2px; margin: 24px 0; border: 0; background: var(--border); }
blockquote { padding: 0 1em; color: var(--muted); border-left: .25em solid var(--border); }
ul, ol { padding-left: 2em; } li + li { margin-top: .25em; }
li > input[type=checkbox] { margin: 0 .3em .2em -1.4em; vertical-align: middle; }
ul:has(> li > input[type=checkbox]) { list-style: none; }
code, kbd, pre { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: 85%; }
code { padding: .2em .4em; border-radius: 6px; background: var(--code-bg); }
pre { padding: 16px; overflow: auto; line-height: 1.45; border-radius: 6px; background: var(--code-bg); }
pre code { padding: 0; background: none; font-size: 100%; }
kbd { padding: 3px 5px; border: 1px solid var(--border); border-bottom-width: 2px; border-radius: 6px; background: var(--code-bg); }
table { border-collapse: collapse; display: block; width: max-content; max-width: 100%; overflow: auto; }
th, td { padding: 6px 13px; border: 1px solid var(--border); }
th { font-weight: 600; } tr:nth-child(2n) { background: #ffffff08; }
img { max-width: 100%; background: transparent; }
details > summary { cursor: pointer; }
.footnotes { font-size: .85em; color: var(--muted); border-top: 1px solid var(--border); padding-top: 8px; }
.abromux-message { color: var(--muted); }
::selection { background: #c9644266; }
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_round_trip_paths_with_spaces_and_unicode() {
        let p = Path::new("/Users/me/My Notes/ünï #1.md");
        let url = url_for(p);
        assert_eq!(
            url,
            "abromux-md://localhost/Users/me/My%20Notes/%C3%BCn%C3%AF%20%231.md"
        );
        assert_eq!(path_of(&url).as_deref(), Some(p));
        assert_eq!(path_of(&format!("{url}#intro")).as_deref(), Some(p));
        assert!(is_markdown_url(&url));
        assert!(!is_markdown_url("https://example.com/a.md"));
        assert_eq!(path_of("https://example.com/a.md"), None);
    }

    #[test]
    fn markdown_files_by_extension() {
        assert!(is_markdown(Path::new("README.md")));
        assert!(is_markdown(Path::new("notes.MARKDOWN")));
        assert!(!is_markdown(Path::new("main.rs")));
        assert!(!is_markdown(Path::new("md")));
    }

    #[test]
    fn renders_gfm_with_github_heading_ids() {
        let html = render(
            "---\ntitle: x\n---\n# Hello, World!\n\n## Hello, World!\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n- [x] done\n\n~~gone~~ <kbd>⌘</kbd>\n",
        );
        assert!(
            !html.contains("title: x"),
            "front matter is left out: {html}"
        );
        assert!(
            html.contains("<h1 id=\"hello-world\">Hello, World!</h1>"),
            "{html}"
        );
        assert!(html.contains("<h2 id=\"hello-world-1\">"), "{html}");
        assert!(html.contains("<table>"));
        assert!(html.contains("type=\"checkbox\""));
        assert!(html.contains("<del>gone</del>"));
        assert!(html.contains("<kbd>⌘</kbd>"));
    }

    #[test]
    fn file_urls_become_the_viewers() {
        let html = render("[a](file:///tmp/a%20b.md) <img src='file://localhost/tmp/x.png'>");
        assert!(
            html.contains("href=\"abromux-md://localhost/tmp/a%20b.md\""),
            "{html}"
        );
        assert!(
            html.contains("src='abromux-md://localhost/tmp/x.png'"),
            "{html}"
        );
    }

    #[test]
    fn heading_ids_use_the_text_not_the_markup() {
        let html = render("### The `browser` command & friends");
        assert!(
            html.contains("id=\"the-browser-command--friends\""),
            "{html}"
        );
    }

    #[test]
    fn serves_markdown_as_a_page_and_text_as_itself() {
        let dir = std::env::temp_dir().join(format!("abromux-md-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let md = dir.join("a b.md");
        std::fs::write(&md, "# Title\n\n<script>alert(1)</script>\n").unwrap();
        std::fs::write(dir.join("x.txt"), "plain").unwrap();
        std::fs::write(dir.join("x.bin"), [0u8, 1, 2]).unwrap();

        let get = |p: &Path| {
            let req = Request::builder().uri(url_for(p)).body(vec![]).unwrap();
            let res = respond(&req);
            let csp = res.headers()["Content-Security-Policy"]
                .to_str()
                .unwrap()
                .to_string();
            (
                res.status().as_u16(),
                res.headers()["Content-Type"].to_str().unwrap().to_string(),
                String::from_utf8_lossy(res.body()).into_owned(),
                csp,
            )
        };
        let (status, mime, body, csp) = get(&md);
        assert_eq!((status, mime.as_str()), (200, "text/html; charset=utf-8"));
        assert!(body.contains("<title>a b.md</title>"));
        assert!(body.contains("<h1 id=\"title\">Title</h1>"));
        assert!(csp.contains("script-src 'none'"));
        assert_eq!(get(&dir.join("x.txt")).2, "plain");
        assert_eq!(get(&dir.join("x.bin")).0, 415);
        assert_eq!(get(&dir.join("gone.md")).0, 404);
        assert!(get(&dir).2.contains("a b.md"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
