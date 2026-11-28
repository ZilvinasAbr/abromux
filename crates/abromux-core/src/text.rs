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

/// `secs` of age in the short form lists use: "now", "5m", "3h", "2d".
pub fn ago_short(secs: i64) -> String {
    match secs.max(0) {
        s if s < 60 => "now".into(),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages() {
        assert_eq!(
            [
                ago_short(-1),
                ago_short(5),
                ago_short(120),
                ago_short(7200),
                ago_short(200_000)
            ],
            ["now", "now", "2m", "2h", "2d"]
        );
    }

    #[test]
    fn descriptions() {
        assert_eq!(clean_description("  \n"), None);
        assert_eq!(
            clean_description("Ship it  \n- build\n\n").as_deref(),
            Some("Ship it\n- build")
        );
    }
}
