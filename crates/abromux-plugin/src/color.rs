//! Hex colours, read the same way everywhere in abromux and its plugins:
//! `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa`, with the `#` optional.

use gpui::Rgba;

/// The colour as `0xrrggbbaa` (opaque forms get `ff`).
pub fn parse_rgba(s: &str) -> Option<u32> {
    let s = s.trim();
    let h = s.strip_prefix('#').unwrap_or(s);
    if !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let full: String = match h.len() {
        3 | 4 => h.chars().flat_map(|c| [c, c]).collect(),
        6 | 8 => h.to_string(),
        _ => return None,
    };
    let n = u32::from_str_radix(&full, 16).ok()?;
    Some(if full.len() == 6 { n << 8 | 0xff } else { n })
}

/// The colour with its alpha, for gpui.
pub fn parse(s: &str) -> Option<Rgba> {
    parse_rgba(s).map(gpui::rgba)
}

/// An opaque colour (`#rgb` or `#rrggbb`) as `0xrrggbb`, for colours that
/// are stored without alpha. A form with alpha is refused.
pub fn parse_rgb(s: &str) -> Option<u32> {
    let s = s.trim();
    let h = s.strip_prefix('#').unwrap_or(s);
    matches!(h.len(), 3 | 6).then(|| parse_rgba(s).map(|n| n >> 8))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forms() {
        assert_eq!(parse_rgba("#ff9500"), Some(0xff9500ff));
        assert_eq!(parse_rgba("0a0"), Some(0x00aa00ff));
        assert_eq!(parse_rgba("#0a08"), Some(0x00aa0088));
        assert_eq!(parse_rgba(" #c96442b0 "), Some(0xc96442b0));
        assert_eq!(parse_rgba("#ff95f"), None);
        assert_eq!(parse_rgba("orange"), None);
        assert_eq!(parse_rgba("#"), None);
        assert_eq!(parse_rgb("#FFaa00"), Some(0xffaa00));
        assert_eq!(parse_rgb("0a0"), Some(0x00aa00));
        assert_eq!(parse_rgb("#ff95"), None);
        assert_eq!(parse_rgb("#c96442b0"), None);
    }
}
