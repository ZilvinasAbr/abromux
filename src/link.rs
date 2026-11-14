//! Links in terminal output (`abromux_core::link`), found in a row of
//! libghostty-vt cells.

use crate::ghostty::Cell;
pub use abromux_core::link::{Local, Target, file_url_path, local};
use std::ops::Range;

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
    abromux_core::link::find(&cells, row.len() as u16, col, cwd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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
