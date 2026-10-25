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
                    String::new()
                } else {
                    c.to_string()
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
