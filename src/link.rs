//! Links in terminal output: the URL or file path under the pointer, for
//! ⌘-click. OSC 8 hyperlinks are looked up by the terminal; this finds the
//! plain-text ones in a row of cells.

use crate::ghostty::Cell;
use std::{ops::Range, path::PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Url(String),
    /// An existing file or directory, with the line from a `path:12` suffix.
    Path(PathBuf, Option<u32>),
}

/// Schemes recognised in plain text (OSC 8 links may use any).
const SCHEMES: &[&str] = &["https://", "http://", "file://"];

/// The link covering column `col` of `row`, and the columns it spans.
/// Relative paths resolve against `cwd`; only paths that exist count.
pub fn find(row: &[Cell], col: u16, cwd: &str) -> Option<(Range<u16>, Target)> {
    // One entry per drawn cell (spacer tails of wide chars are skipped).
    let cells: Vec<(u16, &str)> = row
        .iter()
        .enumerate()
        .filter(|(_, c)| !c.spacer)
        .map(|(x, c)| {
            (
                x as u16,
                if c.text.is_empty() {
                    " "
                } else {
                    c.text.as_str()
                },
            )
        })
        .collect();
    let at = cells.iter().rposition(|&(x, _)| x <= col)?;
    let breaks = |s: &str| {
        s.chars()
            .all(|c| c.is_whitespace() || "\"'`<>│".contains(c))
    };
    if breaks(cells[at].1) {
        return None;
    }
    let mut start = at;
    while start > 0 && !breaks(cells[start - 1].1) {
        start -= 1;
    }
    let mut end = at + 1;
    while end < cells.len() && !breaks(cells[end].1) {
        end += 1;
    }
    let word: Vec<&str> = cells[start..end].iter().map(|c| c.1).collect();
    let (from, to, target) = classify(&word, cwd)?;
    let (first, last) = (start + from, start + to);
    (first..last).contains(&at).then(|| {
        let end_col = cells.get(last).map_or(row.len() as u16, |c| c.0);
        (cells[first].0..end_col, target)
    })
}

/// The link in a whitespace-delimited word (one string per cell), as the
/// cell range it covers and what it points at.
fn classify(word: &[&str], cwd: &str) -> Option<(usize, usize, Target)> {
    let text: String = word.concat();
    // Byte offset → cell index, to map the trimmed link back to cells.
    let mut cell_at = vec![];
    for (i, w) in word.iter().enumerate() {
        cell_at.extend(std::iter::repeat_n(i, w.len()));
    }
    cell_at.push(word.len());

    if let Some(s) = SCHEMES.iter().filter_map(|s| text.find(s)).min() {
        let url = trim_end(&text[s..]);
        return (url.len() > text[s..].find("://")? + 3).then(|| {
            (
                cell_at[s],
                cell_at[s + url.len()],
                Target::Url(url.to_string()),
            )
        });
    }

    let s = text.find(|c: char| !"([{".contains(c))?;
    let path = trim_end(&text[s..]);
    let (file, line) = split_line(path);
    let full = if let Some(rest) = file.strip_prefix("~/") {
        PathBuf::from(std::env::var("HOME").ok()?).join(rest)
    } else {
        PathBuf::from(cwd).join(file)
    };
    // Only paths that exist count, so prose doesn't light up.
    if file.is_empty() || file == "." || !full.exists() {
        return None;
    }
    Some((
        cell_at[s],
        cell_at[s + path.len()],
        Target::Path(full, line),
    ))
}

/// Drop trailing punctuation that ends a sentence rather than the link,
/// keeping a `)` that closes a `(` inside it (Wikipedia-style URLs).
fn trim_end(s: &str) -> &str {
    let mut s = s;
    loop {
        let Some(c) = s.chars().last() else { return s };
        let unbalanced =
            |open: char, close: char| s.matches(close).count() > s.matches(open).count();
        let drop = match c {
            '.' | ',' | ';' | ':' | '!' | '?' => true,
            ')' => unbalanced('(', ')'),
            ']' => unbalanced('[', ']'),
            '}' => unbalanced('{', '}'),
            _ => false,
        };
        if !drop {
            return s;
        }
        s = &s[..s.len() - c.len_utf8()];
    }
}

/// What ⌘-click does with a link that isn't a web page.
#[derive(Debug, PartialEq)]
pub enum Local {
    /// A document or folder: open it with its default app.
    Open(PathBuf),
    /// Something that could run code if opened (an app, a script, an
    /// installer, a bundle or anything in one): show it in Finder instead.
    Reveal(PathBuf),
}

/// Extensions whose default app runs them, or that are bundles of code.
/// Matched on every component, so nothing inside an `.app` opens either.
const RUNS_CODE: &[&str] = &[
    "app",
    "appex",
    "action",
    "applescript",
    "bundle",
    "command",
    "configprofile",
    "dmg",
    "fileloc",
    "framework",
    "inetloc",
    "jar",
    "kext",
    "mdimporter",
    "mobileconfig",
    "mpkg",
    "osax",
    "pkg",
    "plugin",
    "prefpane",
    "qlgenerator",
    "saver",
    "scpt",
    "scptd",
    "shortcut",
    "terminal",
    "tool",
    "url",
    "webloc",
    "wflow",
    "workflow",
    "xpc",
];

/// Where a non-web link may lead. OSC 8 links name their target apart from
/// the text shown, so a program could dress `file:///…/evil.app` or
/// `x-scheme://…` up as an innocent URL: only local `file://` URLs and
/// existing paths count, nothing else goes to `open`, and what could run
/// code is only revealed.
pub fn local(target: &Target) -> Option<Local> {
    let path = match target {
        Target::Path(p, _) => p.clone(),
        Target::Url(u) => file_url_path(u)?,
    };
    if !path.is_absolute() {
        return None;
    }
    // Judge where it really leads: `notes.txt` may be a link to an app.
    let path = path.canonicalize().ok()?;
    let meta = std::fs::metadata(&path).ok()?;
    Some(if runs_code(&path, &meta) {
        Local::Reveal(path)
    } else {
        Local::Open(path)
    })
}

fn runs_code(path: &std::path::Path, meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let risky = path.components().any(|c| {
        std::path::Path::new(c.as_os_str())
            .extension()
            .is_some_and(|e| RUNS_CODE.contains(&e.to_string_lossy().to_lowercase().as_str()))
    });
    // An executable file opens in Terminal and runs.
    risky || (meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

/// The local path a `file://` URL names, percent-decoded. None for other
/// schemes and for files on another host.
fn file_url_path(url: &str) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let rest = url
        .get(..7)
        .filter(|s| s.eq_ignore_ascii_case("file://"))
        .map(|_| &url[7..])?;
    let slash = rest.find('/')?;
    let host = &rest[..slash];
    if !(host.is_empty() || host.eq_ignore_ascii_case("localhost") || is_this_host(host)) {
        return None;
    }
    let path = rest[slash..].split(['?', '#']).next()?;
    let mut bytes = Vec::with_capacity(path.len());
    let mut it = path.bytes();
    while let Some(b) = it.next() {
        if b == b'%' {
            let hex = [it.next()?, it.next()?];
            bytes.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
        } else {
            bytes.push(b);
        }
    }
    if bytes.contains(&0) {
        return None;
    }
    Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}

/// Whether `host` is this Mac's name (shells put it in OSC 8 `file://` links).
fn is_this_host(host: &str) -> bool {
    let mut buf = [0u8; 256];
    if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } != 0 {
        return false;
    }
    let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    let name = String::from_utf8_lossy(&buf[..len]);
    let short = |s: &str| s.split('.').next().unwrap_or_default().to_lowercase();
    name.eq_ignore_ascii_case(host) || (!host.contains('.') && short(&name) == short(host))
}

/// `src/main.rs:12:5` → (`src/main.rs`, Some(12)).
fn split_line(s: &str) -> (&str, Option<u32>) {
    let mut parts = s.splitn(3, ':');
    let file = parts.next().unwrap_or(s);
    match parts.next().map(str::parse::<u32>) {
        Some(Ok(line)) => (file, Some(line)),
        _ => (s, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(s: &str) -> Vec<Cell> {
        s.chars()
            .map(|c| Cell {
                text: if c == ' ' {
                    Default::default()
                } else {
                    c.to_string().into()
                },
                ..Default::default()
            })
            .collect()
    }

    fn url(s: &str) -> Target {
        Target::Url(s.into())
    }

    #[test]
    fn finds_urls_without_the_punctuation_around_them() {
        let r = row("see (https://example.com/a_(b)), then");
        assert_eq!(
            find(&r, 10, "/"),
            Some((5..30, url("https://example.com/a_(b)")))
        );
        assert_eq!(find(&r, 4, "/"), None, "the ( before it isn't part of it");
        assert_eq!(find(&r, 33, "/"), None);
        let r = row("href=\"http://x.io/?q=1\".");
        assert_eq!(find(&r, 12, "/"), Some((6..22, url("http://x.io/?q=1"))));
        assert_eq!(find(&row("https:// alone"), 3, "/"), None);
    }

    #[test]
    fn finds_existing_paths_with_line_numbers() {
        let here = env!("CARGO_MANIFEST_DIR");
        let r = row("error at src/link.rs:42:7: oops");
        let target = Target::Path(PathBuf::from(here).join("src/link.rs"), Some(42));
        assert_eq!(find(&r, 12, here), Some((9..25, target)));
        assert_eq!(
            find(&row("see src/nope.rs here"), 6, here),
            None,
            "missing files aren't links"
        );
        assert_eq!(
            find(&row("Cargo.toml."), 2, here),
            Some((
                0..10,
                Target::Path(PathBuf::from(here).join("Cargo.toml"), None)
            ))
        );
    }

    #[test]
    fn only_local_files_open_and_code_is_only_revealed() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("abromux-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Evil.app/Contents")).unwrap();
        let dir = dir.canonicalize().unwrap();
        let doc = dir.join("notes 1.txt");
        std::fs::write(&doc, "hi").unwrap();
        let script = dir.join("build");
        std::fs::write(&script, "#!/bin/sh").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let cmd = dir.join("run.COMMAND");
        std::fs::write(&cmd, "").unwrap();
        let app = dir.join("Evil.app");
        let alias = dir.join("readme.txt");
        std::os::unix::fs::symlink(&app, &alias).unwrap();
        let file_url = |p: &std::path::Path| format!("file://{}", p.display()).replace(' ', "%20");

        let path = |p: &std::path::Path| local(&Target::Path(p.into(), None));
        assert_eq!(path(&doc), Some(Local::Open(doc.clone())));
        assert_eq!(path(&dir), Some(Local::Open(dir.clone())));
        assert_eq!(path(&script), Some(Local::Reveal(script.clone())));
        assert_eq!(path(&cmd), Some(Local::Reveal(cmd.clone())));
        assert_eq!(path(&app), Some(Local::Reveal(app.clone())));
        assert_eq!(
            path(&app.join("Contents")),
            Some(Local::Reveal(app.join("Contents")))
        );
        assert_eq!(
            path(&alias),
            Some(Local::Reveal(app.clone())),
            "judged by where it leads"
        );

        let url = |u: &str| local(&Target::Url(u.into()));
        assert_eq!(url(&file_url(&doc)), Some(Local::Open(doc.clone())));
        assert_eq!(
            url(&file_url(&doc).replacen("file://", "FILE://localhost", 1)),
            Some(Local::Open(doc.clone()))
        );
        assert_eq!(url(&file_url(&app)), Some(Local::Reveal(app.clone())));
        assert_eq!(
            url(&file_url(&doc).replacen("file://", "file://elsewhere.example", 1)),
            None
        );
        assert_eq!(url(&file_url(&dir.join("missing"))), None);
        assert_eq!(url(&format!("{}%00", file_url(&doc))), None);
        assert_eq!(url("file://relative/x"), None);
        for other in [
            "-aCalculator",
            "vscode://file/etc/passwd",
            "smb://host/share",
            "mailto:x@y.z",
            "x-apple.systempreferences:",
        ] {
            assert_eq!(url(other), None, "{other}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn wide_cells_map_back_to_columns() {
        // 日 takes two columns (a cell and its spacer), so the URL starts at 3.
        let mut r = vec![
            Cell {
                text: "日".into(),
                wide: true,
                ..Default::default()
            },
            Cell {
                spacer: true,
                ..Default::default()
            },
        ];
        r.extend(row(" http://a.b"));
        assert_eq!(find(&r, 5, "/"), Some((3..13, url("http://a.b"))));
    }
}
