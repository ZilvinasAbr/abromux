//! Small text helpers.

/// Its non-blank lines, trimmed, joined with spaces.
pub fn one_line(s: &str) -> String {
    s.split(['\n', '\r'])
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}
