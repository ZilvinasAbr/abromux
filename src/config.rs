//! Reads the user's Ghostty config so terminals look and type like their
//! Ghostty: font, colors, palette, theme, cursor style and `text:`/`csi:`/
//! `esc:` keybinds. Everything else in the file is ignored.
//!
//! Files, in load order (later wins): `$XDG_CONFIG_HOME/ghostty/config`
//! (default `~/.config/ghostty/config`), then
//! `~/Library/Application Support/com.mitchellh.ghostty/config`, each
//! followed by its `config-file` includes. A `theme` is applied underneath.

use crate::ghostty::Rgb;
use gpui::{Global, Keystroke, Modifiers};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorStyle {
    Block,
    Bar,
    Underline,
    BlockHollow,
}

#[derive(Clone, Debug, PartialEq)]
pub struct KeyBind {
    pub key: String,
    pub mods: Modifiers,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub font_family: String,
    pub font_size: f32,
    pub foreground: Option<Rgb>,
    pub background: Option<Rgb>,
    pub cursor_color: Option<Rgb>,
    pub cursor_text: Option<Rgb>,
    pub selection_background: Option<Rgb>,
    pub selection_foreground: Option<Rgb>,
    pub palette: Vec<(u8, Rgb)>,
    pub cursor_style: Option<CursorStyle>,
    pub keybinds: Vec<KeyBind>,
}

impl Global for Config {}

impl Default for Config {
    fn default() -> Self {
        Self {
            font_family: "JetBrains Mono".into(),
            font_size: 14.,
            foreground: None,
            background: None,
            cursor_color: None,
            cursor_text: None,
            selection_background: None,
            selection_foreground: None,
            palette: vec![],
            cursor_style: None,
            keybinds: DEFAULT_KEYBINDS
                .iter()
                .filter_map(|(t, a)| {
                    let (key, mods) = parse_trigger(t)?;
                    Some(KeyBind {
                        key,
                        mods,
                        bytes: parse_action(a)?,
                    })
                })
                .collect(),
        }
    }
}

/// Ghostty's built-in macOS keybinds that send text to the shell, so ⌘⌫,
/// ⌘←/→ and ⌥←/→ edit the line as they do there. `keybind = clear` drops them.
const DEFAULT_KEYBINDS: &[(&str, &str)] = &[
    ("super+backspace", "text:\\x15"),
    ("super+left", "text:\\x01"),
    ("super+right", "text:\\x05"),
    ("alt+left", "esc:b"),
    ("alt+right", "esc:f"),
];

impl Config {
    pub fn load() -> Self {
        let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
        let xdg = std::env::var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".config"));
        let mut entries = vec![];
        for path in [
            xdg.join("ghostty/config"),
            home.join("Library/Application Support/com.mitchellh.ghostty/config"),
        ] {
            read_file(&path, &mut entries, 0);
        }
        let mut theme = vec![];
        if let Some((_, name)) = entries.iter().rev().find(|(k, _)| k == "theme")
            && let Some(path) = find_theme(name, &xdg)
        {
            read_file(&path, &mut theme, 0);
        }
        theme.extend(entries);
        Self::from_entries(&theme)
    }

    fn from_entries(entries: &[(String, String)]) -> Self {
        let mut c = Self::default();
        let mut family: Option<String> = None;
        for (k, v) in entries {
            match k.as_str() {
                // Repeats add fallbacks in Ghostty; the first one is the face.
                "font-family" if v.is_empty() => family = None,
                "font-family" if family.is_none() => family = Some(unquote(v).into()),
                "font-size" => c.font_size = v.parse().unwrap_or(c.font_size),
                "foreground" => c.foreground = parse_color(v),
                "background" => c.background = parse_color(v),
                "cursor-color" => c.cursor_color = parse_color(v),
                "cursor-text" => c.cursor_text = parse_color(v),
                "selection-background" => c.selection_background = parse_color(v),
                "selection-foreground" => c.selection_foreground = parse_color(v),
                "palette" => {
                    if let Some((i, col)) = v.split_once('=')
                        && let (Ok(i), Some(col)) = (i.trim().parse::<u8>(), parse_color(col))
                    {
                        c.palette.retain(|(j, _)| *j != i);
                        c.palette.push((i, col));
                    }
                }
                "cursor-style" => {
                    c.cursor_style = match v.as_str() {
                        "block" => Some(CursorStyle::Block),
                        "bar" => Some(CursorStyle::Bar),
                        "underline" => Some(CursorStyle::Underline),
                        "block_hollow" => Some(CursorStyle::BlockHollow),
                        _ => c.cursor_style,
                    }
                }
                "keybind" if v == "clear" => c.keybinds.clear(),
                "keybind" => {
                    if let Some((trigger, action)) = v.split_once('=')
                        && let Some((key, mods)) = parse_trigger(trigger)
                    {
                        c.keybinds.retain(|b| !(b.key == key && b.mods == mods));
                        if let Some(bytes) = parse_action(action) {
                            c.keybinds.push(KeyBind { key, mods, bytes });
                        }
                    }
                }
                _ => {}
            }
        }
        if let Some(f) = family {
            c.font_family = f;
        }
        c
    }

    /// Bytes bound to this keystroke by a Ghostty `keybind`, if any.
    pub fn keybind(&self, ks: &Keystroke) -> Option<&[u8]> {
        let m = ks.modifiers;
        let mods = Modifiers {
            control: m.control,
            alt: m.alt,
            shift: m.shift,
            platform: m.platform,
            function: false,
        };
        self.keybinds
            .iter()
            .find(|b| b.key == ks.key && b.mods == mods)
            .map(|b| b.bytes.as_slice())
    }
}

fn read_file(path: &Path, out: &mut Vec<(String, String)>, depth: usize) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let mut includes = vec![];
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let (k, v) = (k.trim().to_string(), v.trim().to_string());
        if k == "config-file" {
            includes.push(v);
        } else {
            out.push((k, v));
        }
    }
    // Ghostty loads includes after the file that names them.
    if depth < 8 {
        for inc in includes {
            let inc = unquote(&inc);
            let inc = inc.strip_prefix('?').unwrap_or(inc);
            let p = Path::new(inc);
            let p = if p.is_absolute() {
                p.to_path_buf()
            } else {
                path.parent().unwrap_or(Path::new("/")).join(p)
            };
            read_file(&p, out, depth + 1);
        }
    }
}

/// `theme = Name`, an absolute path, or `light:A,dark:B` (abromux is dark-only).
fn find_theme(value: &str, xdg: &Path) -> Option<PathBuf> {
    let value = unquote(value);
    let name = value
        .split(',')
        .find_map(|p| p.trim().strip_prefix("dark:"))
        .unwrap_or_else(|| {
            value
                .split(',')
                .next()
                .unwrap_or(value)
                .trim()
                .trim_start_matches("light:")
        });
    if Path::new(name).is_absolute() {
        return Some(name.into());
    }
    let mut dirs = vec![xdg.join("ghostty/themes")];
    if let Ok(r) = std::env::var("GHOSTTY_RESOURCES_DIR") {
        dirs.push(PathBuf::from(r).join("themes"));
    }
    dirs.push("/Applications/Ghostty.app/Contents/Resources/ghostty/themes".into());
    dirs.into_iter().map(|d| d.join(name)).find(|p| p.is_file())
}

fn unquote(s: &str) -> &str {
    s.strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(s)
}

pub fn parse_color(s: &str) -> Option<Rgb> {
    let s = unquote(s.trim());
    let hex = s.strip_prefix('#').unwrap_or(s);
    if hex.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(hex, 16).ok()?;
    Some(Rgb((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

/// `shift+enter`, `ctrl+a`, `super+k` → a gpui key name plus modifiers.
/// Leader sequences (`a>b`) aren't supported.
fn parse_trigger(t: &str) -> Option<(String, Modifiers)> {
    let mut t = t.trim();
    while let Some((prefix, rest)) = t.split_once(':') {
        if !matches!(
            prefix,
            "global" | "all" | "unconsumed" | "performable" | "physical"
        ) {
            break;
        }
        t = rest;
    }
    if t.contains('>') {
        return None;
    }
    let mut mods = Modifiers::default();
    let mut key = None;
    for part in t.split('+') {
        match part {
            "shift" => mods.shift = true,
            "ctrl" | "control" => mods.control = true,
            "alt" | "opt" | "option" => mods.alt = true,
            "super" | "cmd" | "command" => mods.platform = true,
            k => key = Some(k),
        }
    }
    let key = match key? {
        "return" => "enter",
        "arrow_up" => "up",
        "arrow_down" => "down",
        "arrow_left" => "left",
        "arrow_right" => "right",
        "page_up" => "pageup",
        "page_down" => "pagedown",
        "esc" => "escape",
        k => k,
    };
    Some((key.to_lowercase(), mods))
}

fn parse_action(a: &str) -> Option<Vec<u8>> {
    let (kind, arg) = a.trim().split_once(':')?;
    let text = zig_unescape(arg);
    Some(match kind {
        "text" => text.into_bytes(),
        "csi" => [b"\x1b[".as_slice(), text.as_bytes()].concat(),
        "esc" => [b"\x1b".as_slice(), text.as_bytes()].concat(),
        _ => return None,
    })
}

/// Zig string-literal escapes, which Ghostty uses for `text:` actions.
fn zig_unescape(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('x') => {
                let hex: String = chars.by_ref().take(2).collect();
                if let Ok(b) = u8::from_str_radix(&hex, 16) {
                    out.push(b as char);
                }
            }
            Some('u') if chars.peek() == Some(&'{') => {
                chars.next();
                let hex: String = chars.by_ref().take_while(|c| *c != '}').collect();
                if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                    out.push(ch);
                }
            }
            Some(o) => out.push(o),
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(s: &str) -> Vec<(String, String)> {
        s.lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.trim().into(), v.trim().into()))
            .collect()
    }

    #[test]
    fn folds_entries_like_ghostty() {
        let c = Config::from_entries(&entries(
            "font-family = \"Berkeley Mono\"\nfont-family = Symbols Nerd Font\nfont-size = 14\n\
             background = #1e1e2e\npalette = 1=#f38ba8\npalette = 1=#ff0000\ncursor-style = bar\n\
             keybind = shift+enter=text:\\n\nkeybind = ctrl+k=csi:2J\nkeybind = super+t=new_tab",
        ));
        assert_eq!(c.font_family, "Berkeley Mono");
        assert_eq!(c.font_size, 14.);
        assert_eq!(c.background, Some(Rgb(0x1e, 0x1e, 0x2e)));
        assert_eq!(c.palette, vec![(1, Rgb(0xff, 0, 0))]);
        assert_eq!(c.cursor_style, Some(CursorStyle::Bar));
        assert_eq!(
            c.keybinds.len(),
            DEFAULT_KEYBINDS.len() + 2,
            "non-text actions are ignored"
        );
        assert_eq!(
            c.keybind(&Keystroke::parse("cmd-backspace").unwrap()),
            Some(b"\x15".as_slice())
        );

        let shift_enter = Keystroke::parse("shift-enter").unwrap();
        assert_eq!(c.keybind(&shift_enter), Some(b"\n".as_slice()));
        assert_eq!(
            c.keybind(&Keystroke::parse("ctrl-k").unwrap()),
            Some(b"\x1b[2J".as_slice())
        );
        assert_eq!(c.keybind(&Keystroke::parse("enter").unwrap()), None);
    }

    #[test]
    fn escapes_and_themes() {
        assert_eq!(zig_unescape(r"a\x1b[0m\u{e9}\\"), "a\x1b[0m\u{e9}\\");
        assert_eq!(parse_color("#FFaa00"), Some(Rgb(255, 170, 0)));
        assert_eq!(parse_color("nope"), None);
        let dir = std::env::temp_dir().join(format!("abromux-theme-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("ghostty/themes")).unwrap();
        std::fs::write(dir.join("ghostty/themes/Night"), "background = #000000\n").unwrap();
        assert_eq!(
            find_theme("light:Day,dark:Night", &dir),
            Some(dir.join("ghostty/themes/Night"))
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
