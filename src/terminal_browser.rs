//! Browser tabs that run terminal-browser (`browser.engine` set to
//! `terminalBrowser`): a terminal whose shell runs `terminal-browser open`,
//! shown with the page's title and URL.
//!
//! abromux learns the page from terminal-browser's control socket: each
//! browser listens on `<state>/terminal-browser-<id>/instances/<key>.sock`
//! and answers `{"cmd":"state"}` with the active tab's `url` and `title` and
//! the `tty` it draws on, which is how abromux matches one to its tab.

use crate::terminal::TerminalView;
use gpui::{AppContext as _, AsyncApp, Entity, Timer};
use serde::Deserialize;
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Duration,
};

/// The shell command that opens `url` in terminal-browser, filling the
/// terminal it runs in, or None if it isn't installed. `--no-merge` keeps it
/// from joining a neighbouring one as a tab: each abromux tab is one browser.
/// abromux shows kitty graphics, so terminal-browser needn't ask. Find,
/// devtools and console take the built-in browser's keys (⌘F, ⌥⌘I, ⌥⌘C),
/// which abromux leaves to the tab (`KEYS`).
pub fn command(url: &str) -> Option<String> {
    let bin = find()?;
    let mut cmd = format!(
        "TERMINAL_BROWSER_SKIP_GRAPHICS_CHECK=1 exec {} open --no-merge \
         --find-key=super+f --devtools-key=super+alt+i --console-key=super+alt+c",
        shell_quote(&bin.to_string_lossy())
    );
    if !url.is_empty() && url != "about:blank" {
        cmd.push(' ');
        cmd.push_str(&shell_quote(url));
    }
    Some(cmd)
}

/// `terminal-browser` on PATH or where its installer puts it; an app
/// opened from the Dock has only the system's PATH.
fn find() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .chain([
            home.join(".local/bin"),
            "/opt/homebrew/bin".into(),
            "/usr/local/bin".into(),
        ])
        .map(|d| d.join("terminal-browser"))
        .find(|p| p.is_file())
}

/// One word for sh, zsh, bash or fish.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// What a terminal-browser shows: its active tab.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Page {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    tty: Option<String>,
}

#[derive(Deserialize)]
struct Reply {
    #[serde(default)]
    ok: bool,
    data: Option<Page>,
}

/// Where terminal-browser keeps its control sockets: under
/// `$XDG_RUNTIME_DIR`, else `$XDG_STATE_HOME` (`~/.local/state`).
fn instance_dirs() -> Vec<PathBuf> {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .or_else(|| std::env::var_os("XDG_STATE_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/state")
        });
    let Ok(entries) = std::fs::read_dir(&base) else {
        return vec![];
    };
    entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("terminal-browser-"))
        .map(|e| e.path().join("instances"))
        .collect()
}

fn sockets() -> Vec<PathBuf> {
    instance_dirs()
        .into_iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "sock"))
        .collect()
}

/// Ask one browser for its page.
fn ask(socket: &Path) -> Option<Page> {
    let mut s = UnixStream::connect(socket).ok()?;
    let timeout = Some(Duration::from_millis(500));
    s.set_read_timeout(timeout).ok()?;
    s.set_write_timeout(timeout).ok()?;
    s.write_all(b"{\"id\":\"abromux\",\"cmd\":\"state\"}\n").ok()?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).ok()?;
    let reply: Reply = serde_json::from_str(&line).ok()?;
    reply.ok.then_some(reply.data).flatten()
}

/// The page of the browser drawing on `tty`, and its socket. `known` is
/// tried first; the others only when it no longer answers for `tty`.
fn find_page(tty: &Path, known: Option<&Path>) -> Option<(PathBuf, Page)> {
    let tty = tty.to_string_lossy();
    let mine = |p: &Page| p.tty.as_deref() == Some(&*tty);
    if let Some(s) = known
        && let Some(p) = ask(s).filter(mine)
    {
        return Some((s.to_path_buf(), p));
    }
    sockets()
        .into_iter()
        .filter(|s| Some(s.as_path()) != known)
        .find_map(|s| ask(&s).filter(mine).map(|p| (s, p)))
}

/// Make `term`, which runs `command(url)`, a terminal-browser tab: it shows
/// a globe and the page's title, keeps the browser's keys, and is saved as a
/// browser tab on the page it shows.
pub fn attach(term: &Entity<TerminalView>, url: &str, cx: &mut gpui::App) {
    term.update(cx, |t, _| {
        t.app = Some(Box::new(Tab {
            url: url.into(),
            title: String::new(),
        }))
    });
    watch(term, cx);
}

/// What a terminal-browser tab shows, as its control socket last said.
pub struct Tab {
    /// At first the page it opened.
    url: String,
    title: String,
}

/// The key context of a terminal-browser tab: `KEYS` are bound to nothing
/// there, so they reach terminal-browser.
pub const KEY_CONTEXT: &str = "TerminalBrowser";

/// The ⌘-keys a terminal-browser tab keeps, as the built-in browser has them:
/// address bar, reload, back/forward, find, devtools and console (`command`
/// starts it with those on ⌘F, ⌥⌘I, ⌥⌘C), zoom, and editing in the page.
/// Window-level keys (⌘T, ⌘W, ⌘P, …) stay abromux's.
pub const KEYS: &[&str] = &[
    "cmd-l",
    "cmd-r",
    "cmd-[",
    "cmd-]",
    "cmd-f",
    "alt-cmd-i",
    "alt-cmd-c",
    "cmd-=",
    "cmd-+",
    "cmd--",
    "cmd-0",
    "cmd-a",
    "cmd-c",
    "cmd-x",
    "cmd-z",
    "cmd-shift-z",
];

impl Tab {
    /// Take in a reported page; true if the tab should redraw.
    fn update(&mut self, page: Page) -> bool {
        let url_moved = !page.url.is_empty() && page.url != self.url;
        if page.title == self.title && !url_moved {
            return false;
        }
        self.title = page.title;
        if url_moved {
            self.url = page.url;
        }
        true
    }
}

impl crate::terminal::TerminalApp for Tab {
    fn icon(&self) -> &'static str {
        "globe"
    }

    fn title(&self) -> String {
        if !self.title.is_empty() {
            self.title.clone()
        } else if !self.url.is_empty() {
            self.url.clone()
        } else {
            "New Tab".into()
        }
    }

    fn key_context(&self) -> &'static str {
        "Terminal TerminalBrowser"
    }

    fn takes_cmd_keys(&self) -> bool {
        true
    }

    fn page_url(&self) -> Option<&str> {
        Some(&self.url)
    }
}

/// Keep `term`'s page title and URL up to date while it runs
/// terminal-browser: often while the first page loads, then once a second.
fn watch(term: &Entity<TerminalView>, cx: &mut gpui::App) {
    let term = term.downgrade();
    cx.spawn(async move |cx: &mut AsyncApp| {
        let mut known: Option<PathBuf> = None;
        for tick in 0usize.. {
            let delay = if tick < 20 { 250 } else { 1000 };
            Timer::after(Duration::from_millis(delay)).await;
            let Ok(tty) = term.read_with(cx, |t, _| t.tty_name()) else {
                return; // the tab is gone
            };
            let Some(tty) = tty else { continue };
            let k = known.clone();
            let found = cx
                .background_spawn(async move { find_page(&tty, k.as_deref()) })
                .await;
            let Some((socket, page)) = found else { continue };
            known = Some(socket);
            let alive = term.update(cx, |t, cx| {
                if t.app_mut::<Tab>().is_some_and(|tab| tab.update(page)) {
                    t.app_changed(cx);
                }
            });
            if alive.is_err() {
                return;
            }
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_parse_and_reach_the_tab() {
        use crate::terminal::TerminalApp;
        for k in KEYS {
            let ks = gpui::Keystroke::parse(k).unwrap_or_else(|e| panic!("{k}: {e}"));
            assert!(ks.modifiers.platform, "{k} is a ⌘-key");
        }
        let tab = Tab {
            url: "https://a.b/".into(),
            title: String::new(),
        };
        let ctx = tab.key_context();
        assert!(ctx.split(' ').any(|c| c == KEY_CONTEXT), "{ctx}");
        assert!(ctx.split(' ').any(|c| c == "Terminal"), "{ctx}");
        assert!(tab.takes_cmd_keys());
    }

    #[test]
    fn tab_follows_the_page() {
        use crate::terminal::TerminalApp;
        let mut tab = Tab {
            url: "https://a.b/".into(),
            title: String::new(),
        };
        assert_eq!(tab.title(), "https://a.b/", "the URL until a title comes");
        let page = |url: &str, title: &str| Page {
            url: url.into(),
            title: title.into(),
            tty: None,
        };
        assert!(tab.update(page("https://a.b/", "A")));
        assert!(!tab.update(page("https://a.b/", "A")), "no change, no redraw");
        assert!(tab.update(page("https://c.d/", "C")));
        assert_eq!((tab.title(), tab.page_url()), ("C".into(), Some("https://c.d/")));
        assert!(!tab.update(page("", "C")), "an empty URL keeps the last one");
    }

    #[test]
    fn quotes_for_the_shell() {
        assert_eq!(shell_quote("https://a.b/?q=1&x"), "'https://a.b/?q=1&x'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn reads_a_control_reply() {
        let r: Reply = serde_json::from_str(
            r#"{"ok":true,"id":"abromux","data":{"url":"https://lrt.lt/","title":"LRT","loading":false,"tty":"/dev/ttys004","tabs":[]}}"#,
        )
        .unwrap();
        let p = r.data.unwrap();
        assert_eq!((p.url.as_str(), p.title.as_str()), ("https://lrt.lt/", "LRT"));
        assert_eq!(p.tty.as_deref(), Some("/dev/ttys004"));
    }

    #[test]
    fn finds_the_browser_on_a_tty() {
        // Short: a socket path has to fit in about 104 bytes.
        let dir = PathBuf::from(format!("/tmp/abx-tb-{}", std::process::id()));
        let inst = dir.join("terminal-browser-t/instances");
        std::fs::create_dir_all(&inst).unwrap();
        let sock = inst.join("k.sock");
        let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        let server = std::thread::spawn(move || {
            for _ in 0..2 {
                let (s, _) = listener.accept().unwrap();
                let mut line = String::new();
                BufReader::new(&s).read_line(&mut line).unwrap();
                assert!(line.contains("\"cmd\":\"state\""), "{line}");
                (&s).write_all(
                    b"{\"ok\":true,\"id\":\"abromux\",\"data\":{\"url\":\"https://a.b/\",\"title\":\"A\",\"tty\":\"/dev/ttys009\"}}\n",
                )
                .unwrap();
            }
        });
        // SAFETY: tests that read XDG_RUNTIME_DIR don't run in parallel with this one.
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", &dir) };
        let (s, p) = find_page(Path::new("/dev/ttys009"), None).unwrap();
        assert_eq!((s.as_path(), p.title.as_str()), (sock.as_path(), "A"));
        assert!(find_page(Path::new("/dev/ttys001"), Some(&sock)).is_none());
        unsafe { std::env::remove_var("XDG_RUNTIME_DIR") };
        server.join().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
