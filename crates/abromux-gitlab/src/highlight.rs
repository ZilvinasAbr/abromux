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
    /// A YAML key, a PHP `$variable`, `<?php`.
    Variable,
    /// A class or type name: `Request`, YAML's `!tag`.
    Type,
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
    /// Lexed by `yaml_line`.
    yaml: bool,
    /// PHP: `$variables`, class names, `#[Attributes]`, keywords in any case.
    php: bool,
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
    yaml: false,
    php: false,
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
    yaml: false,
    php: false,
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
    yaml: false,
    php: false,
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
    yaml: false,
    php: false,
};

const MARKDOWN: Lang = Lang {
    markdown: true,
    plain: false,
    ..PLAIN
};

const YAML: Lang = Lang {
    yaml: true,
    plain: false,
    ..PLAIN
};

const PHP: Lang = Lang {
    keywords: &[
        "abstract", "and", "array", "as", "bool", "break", "callable", "case", "catch", "class", "clone",
        "const", "continue", "declare", "default", "do", "echo", "else", "elseif", "empty", "enddeclare",
        "endfor", "endforeach", "endif", "endswitch", "endwhile", "enum", "extends", "false", "final",
        "finally", "float", "fn", "for", "foreach", "function", "global", "goto", "if", "implements",
        "include", "include_once", "instanceof", "insteadof", "int", "interface", "isset", "iterable",
        "list", "match", "mixed", "namespace", "never", "new", "null", "object", "or", "parent", "print",
        "private", "protected", "public", "readonly", "require", "require_once", "return", "self",
        "static", "string", "switch", "throw", "trait", "true", "try", "unset", "use", "var", "void",
        "while", "xor", "yield",
    ],
    line_comments: &["//", "#"],
    block_comment: Some(("/*", "*/")),
    quotes: &['"', '\'', '`'],
    definers: &["function", "fn"],
    rust_chars: false,
    markdown: false,
    plain: false,
    yaml: false,
    php: true,
};

fn lang_for(path: &str) -> &'static Lang {
    let name = path.rsplit('/').next().unwrap_or(path);
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("rs") => &RUST,
        Some(
            "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "go" | "java" | "kt" | "c" | "h" | "cc" | "cpp"
            | "hpp" | "cs" | "swift" | "scala" | "dart" | "css" | "scss" | "json" | "vue",
        ) => &C_LIKE,
        Some("php" | "phtml" | "php5" | "php7" | "php8" | "phpt") => &PHP,
        Some("yml" | "yaml") => &YAML,
        Some("py" | "rb" | "sh" | "bash" | "zsh" | "toml" | "pl" | "r" | "ex" | "exs")
        | None
            if !matches!(name, "README" | "LICENSE") =>
        {
            &HASH
        }
        Some("md" | "markdown") => &MARKDOWN,
        _ if matches!(name, "Dockerfile" | "Makefile" | "Gemfile" | "Rakefile") => &HASH,
        // `.yml.dist`, `.yaml.example`, `.php.dist`: what's before the last part.
        Some("dist" | "example" | "sample" | "tpl" | "template" | "j2") => {
            let inner = name.rsplit_once('.').map_or(name, |(n, _)| n);
            if inner.contains('.') { lang_for(inner) } else { &PLAIN }
        }
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
        if lang.yaml {
            return yaml_line(text);
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
            // PHP's `#[Attribute]` isn't a `#` comment.
            let attribute = lang.php && rest.starts_with("#[");
            if !attribute && lang.line_comments.iter().any(|c| rest.starts_with(c)) {
                push(&mut out, i..b.len(), Tok::Comment);
                break;
            }
            let c = rest.chars().next().unwrap();
            if lang.php {
                // `<?php`, `<?=` and `?>`.
                let tag = ["<?php", "<?=", "<?", "?>"].iter().find(|t| rest.starts_with(**t));
                if let Some(t) = tag {
                    push(&mut out, i..i + t.len(), Tok::Variable);
                    i += t.len();
                    prev_word = None;
                    continue;
                }
                // `$name`, `$this`.
                if c == '$' {
                    let len = rest[1..].find(|ch: char| !(ch.is_alphanumeric() || ch == '_')).unwrap_or(rest.len() - 1);
                    if len > 0 {
                        push(&mut out, i..i + 1 + len, Tok::Variable);
                        i += 1 + len;
                        prev_word = None;
                        continue;
                    }
                }
            }
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
                let keyword = lang.keywords.contains(&word)
                    || (lang.php && lang.keywords.contains(&word.to_ascii_lowercase().as_str()) && !after.starts_with('('));
                // `->name` and `::name` are members, whatever they're called.
                let member = lang.php && (text[..i].ends_with("->") || text[..i].ends_with("::"));
                let tok = if member {
                    after.starts_with('(').then_some(Tok::Function)
                } else if keyword {
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
                } else if lang.php && word.starts_with(|ch: char| ch.is_uppercase()) && word.chars().any(|ch| ch.is_lowercase()) {
                    // `new Request(`, `Foo $x`, `App\Models\User`: a class.
                    Some(Tok::Type)
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

/// One line of YAML, as Rouge colors it: the key, then its value (quoted or
/// plain text a string, numbers, `true`/`null`, `&anchors`, `*aliases`,
/// `!tags`), and comments. A line with no key (a list item, a block
/// scalar's text) is a value.
fn yaml_line(text: &str) -> Spans {
    let mut out: Spans = vec![];
    let b = text.as_bytes();
    let mut i = text.len() - text.trim_start().len();
    if text[i..].starts_with('#') {
        return vec![(i..text.len(), Tok::Comment)];
    }
    // `---` and `...`: a document's start and end.
    if i == 0 && (text.starts_with("---") || text.trim_end() == "...") {
        let stop = text.find(' ').unwrap_or(text.len());
        out.push((0..stop, Tok::Namespace));
        i = stop;
    }
    // List items: `- - key: value`.
    loop {
        let rest = &text[i..];
        if rest == "-" || rest.starts_with("- ") || rest == "?" || rest.starts_with("? ") {
            push(&mut out, i..i + 1, Tok::Operator);
            i += 1;
            i += text[i..].len() - text[i..].trim_start().len();
        } else {
            break;
        }
    }
    // The key: quoted or plain, up to a `:` that ends the line or has a space after.
    let rest = &text[i..];
    let key_end = match rest.chars().next() {
        Some(q @ ('"' | '\'')) => {
            let end = string_end(text, i, q);
            text[end..].trim_start().starts_with(':').then_some(end)
        }
        Some(c) if !matches!(c, '[' | '{' | '&' | '*' | '!' | '|' | '>' | '#') => {
            let mut found = None;
            for (j, ch) in rest.char_indices() {
                if ch == ':' && rest[j + 1..].chars().next().is_none_or(|n| n == ' ' || n == '\t') {
                    found = Some(i + j);
                    break;
                }
                // A comment starts before any colon: no key.
                if ch == '#' && j > 0 && b[i + j - 1] == b' ' {
                    break;
                }
            }
            found
        }
        _ => None,
    };
    if let Some(end) = key_end {
        if end > i {
            out.push((i..end, Tok::Variable));
        }
        let colon = end + text[end..].find(':').unwrap_or(0);
        out.push((colon..colon + 1, Tok::Operator));
        i = colon + 1;
    }
    yaml_value(text, i, &mut out);
    out
}

/// A YAML value from `start` to the end of the line.
fn yaml_value(text: &str, start: usize, out: &mut Spans) {
    let b = text.as_bytes();
    let mut i = start;
    // Inside `[…]` or `{…}`, commas and brackets split the scalars.
    let mut flow = 0usize;
    while i < b.len() {
        let rest = &text[i..];
        let c = rest.chars().next().unwrap();
        if c.is_whitespace() {
            i += c.len_utf8();
            continue;
        }
        if c == '#' && (i == 0 || b[i - 1].is_ascii_whitespace()) {
            out.push((i..text.len(), Tok::Comment));
            return;
        }
        match c {
            '"' | '\'' => {
                let stop = string_end(text, i, c);
                push(out, i..stop, Tok::String);
                i = stop;
                continue;
            }
            '[' | '{' => {
                flow += 1;
                i += 1;
                continue;
            }
            ']' | '}' if flow > 0 => {
                flow -= 1;
                i += 1;
                continue;
            }
            ',' if flow > 0 => {
                i += 1;
                continue;
            }
            '&' | '*' | '!' => {
                let len = rest.find(|ch: char| ch.is_whitespace() || (flow > 0 && matches!(ch, ',' | ']' | '}'))).unwrap_or(rest.len());
                if len > 1 || c == '!' {
                    out.push((i..i + len, if c == '!' { Tok::Type } else { Tok::Namespace }));
                    i += len;
                    continue;
                }
            }
            '|' | '>' => {
                let len = rest.find(char::is_whitespace).unwrap_or(rest.len());
                if rest[len..].trim().is_empty() || rest[len..].trim_start().starts_with('#') {
                    out.push((i..i + len, Tok::Operator));
                    i += len;
                    continue;
                }
            }
            _ => {}
        }
        // A plain scalar: to the end of the line, a comment, or (in a flow
        // collection) the next `,`, `]`, `}` or `key:`.
        let mut stop = text.len();
        for (j, ch) in rest.char_indices() {
            let comment = ch == '#' && j > 0 && rest.as_bytes()[j - 1] == b' ';
            let split = flow > 0 && (matches!(ch, ',' | ']' | '}') || (ch == ':' && rest[j + 1..].starts_with(' ')));
            if comment || split {
                stop = i + j;
                break;
            }
        }
        let end = i + text[i..stop].trim_end().len();
        if end == i {
            // A lone `:` in a flow mapping.
            i += c.len_utf8();
            continue;
        }
        let word = &text[i..end];
        let in_map_key = flow > 0 && text[stop..].starts_with(':');
        let tok = if in_map_key {
            Tok::Variable
        } else if matches!(word, "true" | "false" | "null" | "~" | "yes" | "no" | "on" | "off" | "True" | "False" | "Null" | "TRUE" | "FALSE" | "NULL") {
            Tok::Keyword
        } else if yaml_number(word) {
            Tok::Number
        } else {
            Tok::String
        };
        out.push((i..end, tok));
        i = stop;
        if in_map_key {
            i += 1;
        }
    }
}

/// `12`, `-1.5`, `0x1f`, `1e3`: what YAML reads as a number.
fn yaml_number(word: &str) -> bool {
    let w = word.strip_prefix(['-', '+']).unwrap_or(word);
    if let Some(hex) = w.strip_prefix("0x") {
        return !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit());
    }
    w.starts_with(|c: char| c.is_ascii_digit()) && w.parse::<f64>().is_ok() || w.replace('_', "").parse::<i64>().is_ok() && !w.is_empty()
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

    #[test]
    fn yaml_keys_and_values() {
        use Tok::*;
        assert_eq!(toks(".gitlab-ci.yml", "image: ruby:3.2 # base"), [("image", Variable), (":", Operator), ("ruby:3.2", String), ("# base", Comment)]);
        assert_eq!(toks("a.yaml", "  - name: \"x: y\""), [("-", Operator), ("name", Variable), (":", Operator), ("\"x: y\"", String)]);
        assert_eq!(toks("a.yml", "replicas: 3"), [("replicas", Variable), (":", Operator), ("3", Number)]);
        assert_eq!(toks("a.yml", "enabled: true"), [("enabled", Variable), (":", Operator), ("true", Keyword)]);
        assert_eq!(toks("a.yml", "stages:"), [("stages", Variable), (":", Operator)]);
        assert_eq!(toks("a.yml", "    - bundle exec rspec"), [("-", Operator), ("bundle exec rspec", String)]);
        assert_eq!(toks("a.yml", "# just a comment"), [("# just a comment", Comment)]);
        assert_eq!(toks("a.yml", "---"), [("---", Namespace)]);
        assert_eq!(toks("a.yml", "base: &base"), [("base", Variable), (":", Operator), ("&base", Namespace)]);
        assert_eq!(toks("a.yml", "<<: *base"), [("<<", Variable), (":", Operator), ("*base", Namespace)]);
        assert_eq!(toks("a.yml", "script: |"), [("script", Variable), (":", Operator), ("|", Operator)]);
        assert_eq!(toks("a.yml", "only: [main, \"dev\"]"), [("only", Variable), (":", Operator), ("main", String), ("\"dev\"", String)]);
        assert_eq!(toks("a.yml", "env: {A: 1, B: x}"), [("env", Variable), (":", Operator), ("A", Variable), ("1", Number), ("B", Variable), ("x", String)]);
        assert_eq!(toks("a.yml", "url: http://x.test/a#b"), [("url", Variable), (":", Operator), ("http://x.test/a#b", String)]);
        assert_eq!(toks("a.yml", "\"quoted key\": v"), [("\"quoted key\"", Variable), (":", Operator), ("v", String)]);
        assert_eq!(toks("a.yml", "tag: !ruby/object x"), [("tag", Variable), (":", Operator), ("!ruby/object", Type), ("x", String)]);
        // `.yml.dist` is YAML too.
        assert_eq!(toks("config.yml.dist", "a: 1"), [("a", Variable), (":", Operator), ("1", Number)]);
        // Odd lines don't panic.
        for l in ["", " ", "-", ":", "a:b", "- ", "?", "key: 'unclosed", "ž: ū", "[", "{a: [1, 2]}", "x: # c", "&", "- !"] {
            let mut h = Highlighter::new("a.yml");
            for (r, _) in h.line(l) {
                assert!(l.get(r).is_some(), "{l:?}");
            }
        }
    }

    #[test]
    fn php_like_gitlab() {
        use Tok::*;
        assert_eq!(toks("a.php", "<?php"), [("<?php", Variable)]);
        assert_eq!(
            toks("a.php", "public function handle(Request $request): ?Response"),
            [("public", Keyword), ("function", Keyword), ("handle", Function), ("Request", Type), ("$request", Variable), ("?", Operator), ("Response", Type)]
        );
        assert_eq!(
            toks("a.php", "$this->repo->save($user); # done"),
            [("$this", Variable), ("->", Operator), ("->", Operator), ("save", Function), ("$user", Variable), ("# done", Comment)]
        );
        assert_eq!(toks("a.php", "#[Route('/x')]"), [("Route", Type), ("'/x'", String)]);
        assert_eq!(toks("a.php", "use App\\Models\\User;"), [("use", Keyword), ("App", Type), ("Models", Type), ("User", Type)]);
        assert_eq!(toks("a.php", "return User::find(1) ?? NULL;"), [("return", Keyword), ("User", Namespace), ("find", Function), ("1", Number), ("??", Operator), ("NULL", Keyword)]);
        assert_eq!(toks("a.php", "$x = array_map(fn ($a) => $a * 2, $list);")[..3], [("$x", Variable), ("=", Operator), ("array_map", Function)]);
        // A member named like a keyword is a member.
        assert_eq!(toks("a.php", "$q->list()"), [("$q", Variable), ("->", Operator), ("list", Function)]);
        assert_eq!(toks("a.php", "self::DEFAULT"), [("self", Keyword)]);
    }
}
