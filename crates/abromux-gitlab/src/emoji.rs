//! GitLab's emoji shortcodes (`:thumbsup:` → 👍) in markdown, for the ones
//! reviews use most. Unknown codes stay as written, as in GitLab.

const CODES: &[(&str, &str)] = &[
    ("+1", "👍"),
    ("-1", "👎"),
    ("thumbsup", "👍"),
    ("thumbsdown", "👎"),
    ("tada", "🎉"),
    ("rocket", "🚀"),
    ("eyes", "👀"),
    ("heart", "❤️"),
    ("smile", "😄"),
    ("smiley", "😃"),
    ("laughing", "😆"),
    ("joy", "😂"),
    ("wink", "😉"),
    ("thinking", "🤔"),
    ("confused", "😕"),
    ("cry", "😢"),
    ("fire", "🔥"),
    ("star", "⭐"),
    ("sparkles", "✨"),
    ("bug", "🐛"),
    ("warning", "⚠️"),
    ("white_check_mark", "✅"),
    ("heavy_check_mark", "✔️"),
    ("x", "❌"),
    ("question", "❓"),
    ("exclamation", "❗"),
    ("bulb", "💡"),
    ("memo", "📝"),
    ("lock", "🔒"),
    ("zap", "⚡"),
    ("wave", "👋"),
    ("pray", "🙏"),
    ("clap", "👏"),
    ("ok_hand", "👌"),
    ("muscle", "💪"),
    ("100", "💯"),
    ("construction", "🚧"),
    ("recycle", "♻️"),
    ("boom", "💥"),
    ("coffee", "☕"),
];

/// The emoji for shortcode `name` (`tada` → 🎉), if it's one we know.
pub fn of(name: &str) -> Option<&'static str> {
    CODES.iter().find(|(k, _)| *k == name).map(|(_, v)| *v)
}

/// What the reaction picker offers: each emoji once, by its main name.
pub fn picks() -> impl Iterator<Item = (&'static str, &'static str)> {
    CODES
        .iter()
        .copied()
        .filter(|(k, _)| !matches!(*k, "+1" | "-1" | "smiley"))
}

/// `text` with each known `:code:` replaced by its emoji.
pub fn expand(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(':') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let end = after.find(':').filter(|&e| {
            e > 0
                && after[..e]
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '+' || c == '-')
        });
        match end.and_then(|e| {
            CODES
                .iter()
                .find(|(k, _)| *k == &after[..e])
                .map(|(_, v)| (e, *v))
        }) {
            Some((e, emoji)) => {
                out.push_str(emoji);
                rest = &after[e + 1..];
            }
            None => {
                out.push(':');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::expand;

    #[test]
    fn expands_known_codes_only() {
        assert_eq!(expand("LGTM :thumbsup: :tada:"), "LGTM 👍 🎉");
        assert_eq!(expand("ratio 1:2:3 and :nope:"), "ratio 1:2:3 and :nope:");
        assert_eq!(expand("std::env::var"), "std::env::var");
        assert_eq!(expand(":+1::-1:"), "👍👎");
        assert_eq!(expand("a: b"), "a: b");
    }
}
