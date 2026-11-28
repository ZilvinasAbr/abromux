//! Small text helpers.

/// Its non-blank lines, trimmed, joined with spaces.
pub fn one_line(s: &str) -> String {
    s.split(['\n', '\r'])
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A description as typed: trailing blank lines and spaces dropped, empty
/// for none.
pub fn clean_description(s: &str) -> Option<String> {
    let lines: Vec<&str> = s.lines().map(str::trim_end).collect();
    let text = lines.join("\n").trim().to_string();
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptions() {
        assert_eq!(clean_description("  \n"), None);
        assert_eq!(
            clean_description("Ship it  \n- build\n\n").as_deref(),
            Some("Ship it\n- build")
        );
    }
}
