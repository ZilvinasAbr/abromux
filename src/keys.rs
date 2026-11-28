//! Keystrokes as config.json writes them: gpui's spelling (`cmd-shift-p`)
//! or macOS's symbols (`⌘⇧P`), several separated by spaces for a chord
//! (`ctrl-a h`), and `when` key contexts. Only the syntax: which actions
//! there are and what they're bound to is `crate::keymap`.

use gpui::{KeyBindingContextPredicate, Keystroke};

/// Named keys gpui knows, besides single characters.
const NAMED_KEYS: &[&str] = &[
    "enter",
    "escape",
    "tab",
    "space",
    "backspace",
    "delete",
    "insert",
    "home",
    "end",
    "pageup",
    "pagedown",
    "left",
    "right",
    "up",
    "down",
];

/// A US keyboard's shifted punctuation. macOS reports ⌘⇧] as cmd-}, so a
/// shortcut written with shift and the unshifted key is folded the same way.
pub(crate) const SHIFTED: &[(&str, &str)] = &[
    ("`", "~"),
    ("-", "_"),
    ("=", "+"),
    ("[", "{"),
    ("]", "}"),
    ("\\", "|"),
    (";", ":"),
    ("'", "\""),
    (",", "<"),
    (".", ">"),
    ("/", "?"),
];

/// A shortcut as the config file writes it, a keystroke or a chord of them
/// separated by spaces, parsed into what macOS will report.
pub fn parse_seq(s: &str) -> Result<Vec<Keystroke>, String> {
    let keys: Vec<Keystroke> = s
        .split_whitespace()
        .map(parse_key)
        .collect::<Result<_, _>>()?;
    if keys.is_empty() {
        return Err("empty shortcut".into());
    }
    Ok(keys)
}

fn parse_key(s: &str) -> Result<Keystroke, String> {
    let bad = || format!("{s:?} isn't a keystroke; write it like \"cmd-shift-p\" or \"⌘⇧P\"");
    // ⌘⇧P → cmd-shift-p.
    let mut dashed = String::new();
    let mut rest = s;
    while let Some(c) = rest.chars().next() {
        let m = match c {
            '⌘' => "cmd",
            '⌥' => "alt",
            '⌃' => "ctrl",
            '⇧' => "shift",
            _ => break,
        };
        dashed.push_str(m);
        dashed.push('-');
        rest = &rest[c.len_utf8()..];
    }
    let rest = match rest {
        "↩" => "enter",
        "⎋" => "escape",
        "⇥" => "tab",
        "⌫" => "backspace",
        "←" => "left",
        "→" => "right",
        "↑" => "up",
        "↓" => "down",
        r => r,
    };
    // Other spellings of the modifiers.
    for part in rest.split_inclusive('-') {
        let (word, dash) = part.strip_suffix('-').map_or((part, ""), |w| (w, "-"));
        let word = match word.to_ascii_lowercase().as_str() {
            "command" => "cmd".to_string(),
            "option" | "opt" => "alt".to_string(),
            "control" => "ctrl".to_string(),
            // gpui reads a capital letter as shift plus the letter; here it's just the letter.
            w if word.len() == 1 => w.to_string(),
            _ => word.to_string(),
        };
        dashed.push_str(&word);
        dashed.push_str(dash);
    }
    let mut k = Keystroke::parse(&dashed).map_err(|_| bad())?;
    let named = NAMED_KEYS.contains(&k.key.as_str())
        || k.key
            .strip_prefix('f')
            .and_then(|n| n.parse::<u8>().ok())
            .is_some_and(|n| (1..=20).contains(&n));
    if k.key.chars().count() != 1 && !named {
        return Err(bad());
    }
    if k.modifiers.shift
        && let Some((_, shifted)) = SHIFTED.iter().find(|(plain, _)| *plain == k.key)
    {
        k.modifiers.shift = false;
        k.key = shifted.to_string();
    }
    k.key_char = None;
    Ok(k)
}

/// Checks a `when` key context, like `Terminal` or `Browser > Input`.
pub fn check_context(s: &str) -> Result<(), String> {
    KeyBindingContextPredicate::parse(s)
        .map(|_| ())
        .map_err(|e| format!("{s:?} isn't a key context: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_keys() {
        assert!(parse_seq("").is_err());
        assert!(parse_seq("cmd-foo").is_err());
        assert!(parse_seq("cmdd-p").is_err());
        assert!(check_context("Terminal").is_ok());
        assert!(check_context("Browser > Input").is_ok());
        assert!(check_context("Terminal &&").is_err());
    }
}
