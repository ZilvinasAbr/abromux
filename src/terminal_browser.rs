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
/// abromux shows kitty graphics, so terminal-browser needn't ask.
pub fn command(url: &str) -> Option<String> {
    let bin = find()?;
    let mut cmd = format!(
        "TERMINAL_BROWSER_SKIP_GRAPHICS_CHECK=1 exec {} open --no-merge",
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

/// Keep `term`'s page title and URL up to date while it runs
/// terminal-browser: often while the first page loads, then once a second.
pub fn watch(term: &Entity<TerminalView>, cx: &mut gpui::App) {
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
            let alive = term.update(cx, |t, cx| t.set_browser_page(page, cx));
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
