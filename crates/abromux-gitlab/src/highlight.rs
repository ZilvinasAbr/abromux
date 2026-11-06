//! Syntax highlighting for diff lines, the way GitLab's web UI does it: a
//! regex-style lexer (GitLab uses Rouge), not a parser, so tokens match
//! what reviewers see there. Lines are lexed one at a time; only block
//! comments carry over to the next line.

use std::ops::Range;

/// Rouge's token classes that GitLab's dark theme colors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tok {
    Keyword,
    String,
    Comment,
    Number,
    /// A call or a definition: `new(`, `fn load`
    Function,
    /// Before `::`: `std::`, `HashMap::`
    Namespace,
    Operator,
    /// `println!`
    Macro,
    /// A markdown heading line.
    Heading,
}

pub type Spans = Vec<(Range<usize>, Tok)>;

struct Lang {
    keywords: &'static [&'static str],
    line_comments: &'static [&'static str],
    block_comment: Option<(&'static str, &'static str)>,
    /// Quote characters that start a string.
    quotes: &'static [char],
    /// Keywords after which a name is a function's: `fn`, `def`.
    definers: &'static [&'static str],
    /// `'` is a lifetime unless it closes within a couple of characters.
    rust_chars: bool,
    markdown: bool,
    /// Not highlighted at all.
    plain: bool,
}

const RUST: Lang = Lang {
    keywords: &[
        "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
        "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
        "ref", "return", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where",
        "while", "Self",
    ],
    line_comments: &["//"],
    block_comment: Some(("/*", "*/")),
    quotes: &['"', '\''],
    definers: &["fn"],
    rust_chars: true,
    markdown: false,
    plain: false,
};

const C_LIKE: Lang = Lang {
    keywords: &[
        "abstract", "async", "await", "break", "case", "catch", "class", "const", "continue", "default",
        "defer", "delete", "do", "else", "enum", "export", "extends", "false", "final", "finally", "for",
        "func", "function", "go", "if", "implements", "import", "in", "instanceof", "interface", "let",
        "map", "new", "nil", "null", "package", "private", "protected", "public", "range", "return",
        "static", "struct", "super", "switch", "this", "throw", "true", "try", "type", "typeof",
        "undefined", "var", "void", "while", "yield", "int", "char", "long", "unsigned", "include",
        "namespace", "template", "typename", "using", "virtual", "override", "fun", "val", "when",
    ],
    line_comments: &["//"],
    block_comment: Some(("/*", "*/")),
    quotes: &['"', '\'', '`'],
    definers: &["function", "func", "fun"],
    rust_chars: false,
    markdown: false,
    plain: false,
};

const HASH: Lang = Lang {
    keywords: &[
        "and", "as", "assert", "begin", "break", "case", "class", "continue", "def", "do", "done", "elif",
        "else", "elsif", "end", "ensure", "esac", "except", "export", "False", "fi", "finally", "for",
        "from", "function", "if", "import", "in", "lambda", "local", "module", "next", "nil", "None",
        "not", "or", "pass", "raise", "require", "rescue", "return", "self", "then", "True", "try",
        "unless", "until", "while", "with", "yield", "true", "false", "null",
    ],
    line_comments: &["#"],
    block_comment: None,
    quotes: &['"', '\'', '`'],
    definers: &["def", "function"],
    rust_chars: false,
    markdown: false,
    plain: false,
};

const PLAIN: Lang = Lang {
    keywords: &[],
    line_comments: &[],
    block_comment: None,
    quotes: &[],
    definers: &[],
    rust_chars: false,
    markdown: false,
    plain: true,
};

const MARKDOWN: Lang = Lang {
    markdown: true,
    plain: false,
    ..PLAIN
};

fn lang_for(path: &str) -> &'static Lang {
    let name = path.rsplit('/').next().unwrap_or(path);
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("rs") => &RUST,
        Some(
            "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "go" | "java" | "kt" | "c" | "h" | "cc" | "cpp"
            | "hpp" | "cs" | "swift" | "scala" | "dart" | "php" | "css" | "scss" | "json" | "vue",
        ) => &C_LIKE,
        Some("py" | "rb" | "sh" | "bash" | "zsh" | "yml" | "yaml" | "toml" | "pl" | "r" | "ex" | "exs")
        | None
            if !matches!(name, "README" | "LICENSE") =>
        {
            &HASH
        }
        Some("md" | "markdown") => &MARKDOWN,
        _ if matches!(name, "Dockerfile" | "Makefile" | "Gemfile" | "Rakefile") => &HASH,
        _ => &PLAIN,
    }
}

/// Lexes the lines of one file's diff, in order.
pub struct Highlighter {
    lang: &'static Lang,
    in_block: bool,
}

impl Highlighter {
    pub fn new(path: &str) -> Self {
        Self {
            lang: lang_for(path),
            in_block: false,
        }
    }

    pub fn line(&mut self, text: &str) -> Spans {
        let lang = self.lang;
        let mut out: Spans = vec![];
        if lang.plain {
            return out;
        }
        if lang.markdown {
            if text.starts_with('#') {
                return vec![(0..text.len(), Tok::Heading)];
            }
            let mut start = None;
            for (i, c) in text.char_indices() {
                if c == '`' {
                    match start.take() {
                        Some(s) => out.push((s..i + 1, Tok::String)),
                        None => start = Some(i),
                    }
                }
            }
            return out;
        }
        let b = text.as_bytes();
        let mut i = 0;
        // The previous word, for `fn name`.
        let mut prev_word: Option<&str> = None;
        while i < b.len() {
            let rest = &text[i..];
            if self.in_block {
                let (_, end) = lang.block_comment.unwrap_or(("", ""));
                let stop = match rest.find(end) {
                    Some(e) => {
                        self.in_block = false;
                        i + e + end.len()
                    }
                    None => b.len(),
                };
                push(&mut out, i..stop, Tok::Comment);
                i = stop;
                continue;
            }
            if let Some((open, _)) = lang.block_comment
                && rest.starts_with(open)
            {
                self.in_block = true;
                // Close on the same line?
                let (_, close) = lang.block_comment.unwrap();
                if let Some(e) = rest[open.len()..].find(close) {
                    self.in_block = false;
                    let stop = i + open.len() + e + close.len();
                    push(&mut out, i..stop, Tok::Comment);
                    i = stop;
                } else {
                    push(&mut out, i..b.len(), Tok::Comment);
                    i = b.len();
                }
                continue;
            }
            if lang.line_comments.iter().any(|c| rest.starts_with(c)) {
                push(&mut out, i..b.len(), Tok::Comment);
                break;
            }
            let c = rest.chars().next().unwrap();
            if lang.quotes.contains(&c) {
                if c == '\'' && lang.rust_chars && !closes_soon(rest) {
                    // A lifetime: 'a
                    i += 1;
                    continue;
                }
                let stop = string_end(text, i, c);
                push(&mut out, i..stop, Tok::String);
                i = stop;
                prev_word = None;
                continue;
            }
            if c.is_ascii_digit() {
                let stop = i + rest
                    .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '.'))
                    .unwrap_or(rest.len());
                push(&mut out, i..stop, Tok::Number);
                i = stop;
                continue;
            }
            if c.is_alphabetic() || c == '_' {
                let stop = i + rest
                    .find(|ch: char| !(ch.is_alphanumeric() || ch == '_'))
                    .unwrap_or(rest.len());
                let word = &text[i..stop];
                let after = &text[stop..];
                let tok = if lang.keywords.contains(&word) {
                    Some(Tok::Keyword)
                } else if prev_word.is_some_and(|p| lang.definers.contains(&p)) {
                    Some(Tok::Function)
                } else if lang.rust_chars && after.starts_with('!') && !after.starts_with("!=") {
                    push(&mut out, i..stop + 1, Tok::Macro);
                    i = stop + 1;
                    prev_word = None;
                    continue;
                } else if after.starts_with("::") {
                    Some(Tok::Namespace)
                } else if after.starts_with('(') {
                    Some(Tok::Function)
                } else {
                    None
                };
                if let Some(t) = tok {
                    push(&mut out, i..stop, t);
                }
                prev_word = Some(word);
                i = stop;
                continue;
            }
            if "<>=!+-*/%&|^~?".contains(c) {
                push(&mut out, i..i + 1, Tok::Operator);
            }
            if !c.is_whitespace() {
                prev_word = None;
            }
            i += c.len_utf8();
        }
        out
    }
}

/// Merge with the previous span when it's the same token right before.
fn push(out: &mut Spans, r: Range<usize>, t: Tok) {
    if let Some((last, lt)) = out.last_mut()
        && *lt == t
        && last.end == r.start
    {
        last.end = r.end;
        return;
    }
    out.push((r, t));
}

/// `'x'` or `'\n'`, not a lifetime.
fn closes_soon(rest: &str) -> bool {
    let b = rest.as_bytes();
    (b.len() > 2 && b[2] == b'\'') || (b.len() > 3 && b[1] == b'\\' && b[3] == b'\'')
        || rest.chars().nth(2) == Some('\'')
}

/// Where the string starting at `start` with `quote` ends (after its
/// closing quote, or at the end of the line).
fn string_end(text: &str, start: usize, quote: char) -> usize {
    let mut escaped = false;
    for (j, ch) in text[start + 1..].char_indices() {
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == quote {
            return start + 1 + j + 1;
        }
    }
    text.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(path: &str, line: &str) -> Vec<(&'static str, Tok)> {
        let mut h = Highlighter::new(path);
        // Leak for a 'static &str in test output: fine in tests.
        let line: &'static str = Box::leak(line.to_string().into_boxed_str());
        h.line(line).into_iter().map(|(r, t)| (&line[r], t)).collect()
    }

    #[test]
    fn rust_like_gitlab() {
        assert_eq!(
            toks("a.rs", "    Config { limit: std::env::var(\"LIMIT\").ok().unwrap_or(100) }"),
            [
                ("std", Tok::Namespace),
                ("env", Tok::Namespace),
                ("var", Tok::Function),
                ("\"LIMIT\"", Tok::String),
                ("ok", Tok::Function),
                ("unwrap_or", Tok::Function),
                ("100", Tok::Number),
            ]
        );
        assert_eq!(
            toks("a.rs", "pub fn load() -> Config {"),
            [("pub", Tok::Keyword), ("fn", Tok::Keyword), ("load", Tok::Function), ("->", Tok::Operator)]
        );
        assert_eq!(toks("a.rs", "/// Allows `x`"), [("/// Allows `x`", Tok::Comment)]);
        assert_eq!(toks("a.rs", "println!(\"{}\", x)"), [("println!", Tok::Macro), ("\"{}\"", Tok::String)]);
        // A lifetime isn't a string; a char is.
        let t = toks("a.rs", "fn f<'a>(c: char) { 'x' }");
        assert_eq!(t.iter().filter(|(_, k)| *k == Tok::String).collect::<Vec<_>>(), [&("'x'", Tok::String)]);
    }

    #[test]
    fn block_comments_span_lines() {
        let mut h = Highlighter::new("x.ts");
        assert_eq!(h.line("a /* b"), [(2..6, Tok::Comment)]);
        assert_eq!(h.line("still"), [(0..5, Tok::Comment)]);
        assert_eq!(h.line("end */ return"), [(0..6, Tok::Comment), (7..13, Tok::Keyword)]);
    }

    #[test]
    fn markdown_and_hash_languages() {
        assert_eq!(toks("README.md", "# Title"), [("# Title", Tok::Heading)]);
        assert_eq!(toks("README.md", "Set `LIMIT` here"), [("`LIMIT`", Tok::String)]);
        assert_eq!(toks("x.py", "def go(): # hi"), [("def", Tok::Keyword), ("go", Tok::Function), ("# hi", Tok::Comment)]);
        assert_eq!(toks("notes.txt", "fn x() // y"), []);
    }
}
