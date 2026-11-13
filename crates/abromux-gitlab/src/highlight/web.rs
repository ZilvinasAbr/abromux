//! The web's languages, colored the way Rouge colors them for GitLab:
//! JavaScript and TypeScript (JSX too), CSS, SCSS and Less, JSON, and
//! HTML, Vue and Svelte with their `<script>` and `<style>`. Line by line
//! like the rest of highlight.rs; what carries over to the next line (a
//! block comment, a template string, a JSX element, a `<style>` block) is a
//! stack of modes.

use super::{Spans, Tok, push, string_end};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Script {
        ts: bool,
        jsx: bool,
    },
    Css {
        scss: bool,
    },
    Json,
    /// HTML, XML, Vue, Svelte.
    Markup,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Until {
    /// The file's own language: never ends.
    End,
    /// `${…}` or a JSX `{…}`: the `}` that closes it.
    Brace,
    /// HTML's `<script>` or `<style>`: its closing tag.
    Close(&'static str),
}

#[derive(Clone, Copy, Debug)]
struct Script {
    ts: bool,
    jsx: bool,
    until: Until,
    /// `{` opened inside a `Brace` script, still to close.
    depth: u32,
    /// Inside `${…}`: its braces are colored.
    interp: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stmt {
    Selector,
    /// A declaration's value, after its `:`.
    Value,
    /// `@media …`, `@include …`.
    At,
}

#[derive(Clone, Copy, Debug)]
struct Css {
    scss: bool,
    until: Until,
    depth: u32,
    /// What the statement being read is; `None` at its start.
    stmt: Option<Stmt>,
}

#[derive(Clone, Debug, Default)]
struct Tag {
    jsx: bool,
    /// `None` until read.
    name: Option<String>,
    closing: bool,
    /// The last attribute's name, and the `lang`/`type` given.
    attr: String,
    lang: String,
}

#[derive(Clone, Debug)]
enum Mode {
    Script(Script),
    Template {
        ts: bool,
        jsx: bool,
    },
    /// A block comment, until this.
    Comment(&'static str),
    Tag(Tag),
    /// An attribute's value that goes on past the line.
    AttrString(char),
    /// JSX's children, or HTML's text.
    Text {
        jsx: bool,
    },
    Css(Css),
    Json,
}

pub struct Lexer {
    stack: Vec<Mode>,
    /// The file is TypeScript: for JSX's `{…}`.
    ts: bool,
    /// The script's last token was a value (`x`, `1`, `)`): a `/` after it
    /// divides and a `<` compares.
    after_value: bool,
}

const KEYWORDS: &[&str] = &[
    "abstract",
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "interface",
    "let",
    "new",
    "null",
    "override",
    "private",
    "protected",
    "public",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "undefined",
    "var",
    "void",
    "while",
    "with",
    "yield",
    "NaN",
    "Infinity",
];
/// Keywords that are values: a `/` after them divides.
const VALUES: &[&str] = &[
    "this",
    "super",
    "true",
    "false",
    "null",
    "undefined",
    "NaN",
    "Infinity",
];
/// Keywords only where one is used as such (`import x from 'y'`), not
/// where it's a name (`const type = 1`, `{ from: a }`).
const SOFT: &[&str] = &[
    "accessor",
    "as",
    "asserts",
    "declare",
    "from",
    "get",
    "infer",
    "is",
    "keyof",
    "module",
    "namespace",
    "of",
    "readonly",
    "satisfies",
    "set",
    "type",
    "unique",
];
/// TypeScript's built-in types.
const TYPES: &[&str] = &[
    "any", "bigint", "boolean", "never", "number", "object", "string", "symbol", "unknown",
];

impl Lexer {
    pub fn new(kind: Kind) -> Self {
        let (base, ts) = match kind {
            Kind::Script { ts, jsx } => (
                Mode::Script(Script {
                    ts,
                    jsx,
                    until: Until::End,
                    depth: 0,
                    interp: false,
                }),
                ts,
            ),
            Kind::Css { scss } => (
                Mode::Css(Css {
                    scss,
                    until: Until::End,
                    depth: 0,
                    stmt: None,
                }),
                false,
            ),
            Kind::Json => (Mode::Json, false),
            Kind::Markup => (Mode::Text { jsx: false }, false),
        };
        Self {
            stack: vec![base],
            ts,
            after_value: false,
        }
    }

    pub fn line(&mut self, text: &str) -> Spans {
        let mut out: Spans = vec![];
        // A line of only `)` or `);` can't be JSX's text: the element was
        // misread (or began before the hunk). Back to the script.
        if matches!(self.stack.last(), Some(Mode::Text { jsx: true })) {
            let t = text.trim();
            if t.starts_with(')') && t.len() <= 3 {
                while self.stack.len() > 1 && !matches!(self.stack.last(), Some(Mode::Script(_))) {
                    self.stack.pop();
                }
            }
        }
        let mut i = 0;
        // Every step moves on or changes the mode; this is only a backstop.
        let mut steps = 0;
        while i < text.len() && steps < text.len() * 4 + 64 {
            steps += 1;
            i = self.step(text, i, &mut out);
        }
        out
    }

    fn step(&mut self, text: &str, i: usize, out: &mut Spans) -> usize {
        match self.stack.last().expect("a base mode") {
            Mode::Script(s) => {
                let s = *s;
                self.script(text, i, out, s)
            }
            Mode::Template { ts, jsx } => {
                let (ts, jsx) = (*ts, *jsx);
                self.template(text, i, out, ts, jsx)
            }
            Mode::Comment(close) => {
                let close = *close;
                match text[i..].find(close) {
                    Some(e) => {
                        push(out, i..i + e + close.len(), Tok::Comment);
                        self.pop();
                        i + e + close.len()
                    }
                    None => {
                        push(out, i..text.len(), Tok::Comment);
                        text.len()
                    }
                }
            }
            Mode::Tag(_) => self.tag(text, i, out),
            Mode::AttrString(q) => {
                let q = *q;
                match text[i..].find(q) {
                    Some(e) => {
                        push(out, i..i + e + 1, Tok::String);
                        self.pop();
                        i + e + 1
                    }
                    None => {
                        push(out, i..text.len(), Tok::String);
                        text.len()
                    }
                }
            }
            Mode::Text { jsx } => {
                let jsx = *jsx;
                self.text(text, i, out, jsx)
            }
            Mode::Css(s) => {
                let s = *s;
                self.css(text, i, out, s)
            }
            Mode::Json => self.json(text, i, out),
        }
    }

    fn pop(&mut self) -> Option<Mode> {
        if self.stack.len() > 1 {
            self.stack.pop()
        } else {
            None
        }
    }

    /// A block comment opening at `i` (`open` long).
    fn block_comment(
        &mut self,
        text: &str,
        i: usize,
        out: &mut Spans,
        open: usize,
        close: &'static str,
    ) -> usize {
        match text[i + open..].find(close) {
            Some(e) => {
                let stop = i + open + e + close.len();
                push(out, i..stop, Tok::Comment);
                stop
            }
            None => {
                push(out, i..text.len(), Tok::Comment);
                self.stack.push(Mode::Comment(close));
                text.len()
            }
        }
    }

    fn script(&mut self, text: &str, i: usize, out: &mut Spans, s: Script) -> usize {
        let rest = &text[i..];
        let c = rest.chars().next().unwrap();
        let next = rest[c.len_utf8()..].chars().next();
        if let Until::Close(tag) = s.until
            && starts_ci(rest, tag)
        {
            self.pop();
            return i;
        }
        if c.is_whitespace() {
            return i + c.len_utf8();
        }
        if rest.starts_with("//") || (i == 0 && rest.starts_with("#!")) {
            push(out, i..text.len(), Tok::Comment);
            return text.len();
        }
        if rest.starts_with("/*") {
            return self.block_comment(text, i, out, 2, "*/");
        }
        let value = std::mem::replace(&mut self.after_value, false);
        match c {
            '"' | '\'' => {
                let stop = string_end(text, i, c);
                push(out, i..stop, Tok::String);
                self.after_value = true;
                return stop;
            }
            '`' => {
                push(out, i..i + 1, Tok::String);
                self.stack.push(Mode::Template {
                    ts: s.ts,
                    jsx: s.jsx,
                });
                return i + 1;
            }
            '{' => {
                if let Some(Mode::Script(s)) = self.stack.last_mut()
                    && s.until == Until::Brace
                {
                    s.depth += 1;
                }
                return i + 1;
            }
            '}' => {
                if s.until == Until::Brace {
                    if s.depth == 0 {
                        if s.interp {
                            push(out, i..i + 1, Tok::Operator);
                        }
                        self.pop();
                    } else if let Some(Mode::Script(s)) = self.stack.last_mut() {
                        s.depth -= 1;
                    }
                }
                return i + 1;
            }
            ')' | ']' => {
                self.after_value = true;
                return i + 1;
            }
            '/' if !value => {
                if let Some(stop) = regex_end(text, i) {
                    push(out, i..stop, Tok::String);
                    self.after_value = true;
                    return stop;
                }
            }
            // JSX: `<div`, `<>`, `</div>`.
            '<' if s.jsx
                && !value
                && next.is_some_and(|n| n.is_ascii_alphabetic() || n == '>' || n == '/') =>
            {
                self.stack.push(Mode::Tag(Tag {
                    jsx: true,
                    ..Tag::default()
                }));
                return i + 1;
            }
            // A decorator: `@Component(`, `@Input()`.
            '@' if next.is_some_and(ident_start) => {
                let n = 1 + ident_len(&rest[1..]);
                push(out, i..i + n, Tok::Type);
                return i + n;
            }
            _ => {}
        }
        if c.is_ascii_digit() || (c == '.' && next.is_some_and(|n| n.is_ascii_digit())) {
            let n = rest
                .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '.'))
                .unwrap_or(rest.len());
            push(out, i..i + n, Tok::Number);
            self.after_value = true;
            return i + n;
        }
        if ident_start(c) {
            return self.script_word(text, i, out);
        }
        if "<>=!+-*/%&|^~?".contains(c) {
            push(out, i..i + 1, Tok::Operator);
        }
        i + c.len_utf8()
    }

    fn script_word(&mut self, text: &str, i: usize, out: &mut Spans) -> usize {
        let stop = i + ident_len(&text[i..]);
        let word = &text[i..stop];
        let after = &text[stop..];
        let before = text[..i].trim_end();
        let member = before.ends_with('.') && !before.ends_with("...");
        let prev = last_word(before);
        // `f(`, `f?.(`, a tagged template `` sql`…` ``, a generic call `f<T>(`.
        let generic = after.starts_with('<')
            && after.find('>').is_some_and(|p| {
                after[p + 1..].starts_with('(') && !after[..p].contains(['(', ')', ';', '='])
            });
        let call = after.starts_with(['(', '`']) || after.starts_with("?.(") || generic;
        self.after_value = true;
        let tok = if member {
            call.then_some(Tok::Function)
        } else if KEYWORDS.contains(&word) {
            self.after_value = VALUES.contains(&word);
            Some(Tok::Keyword)
        } else if SOFT.contains(&word) && soft_keyword(after) {
            self.after_value = false;
            Some(Tok::Keyword)
        } else if TYPES.contains(&word) && !after.trim_start().starts_with([':', '(', '.']) {
            Some(Tok::Keyword)
        } else if prev == "function" || arrow_def(after) {
            Some(Tok::Function)
        } else if matches!(
            prev,
            "class"
                | "interface"
                | "type"
                | "enum"
                | "extends"
                | "implements"
                | "namespace"
                | "new"
        ) {
            Some(Tok::Type)
        } else if call {
            Some(Tok::Function)
        } else if word.starts_with(|c: char| c.is_ascii_uppercase())
            && word.chars().any(|c| c.is_ascii_lowercase())
        {
            Some(Tok::Type)
        } else {
            None
        };
        if let Some(t) = tok {
            push(out, i..stop, t);
        }
        stop
    }

    fn template(&mut self, text: &str, i: usize, out: &mut Spans, ts: bool, jsx: bool) -> usize {
        let b = text.as_bytes();
        let mut j = i;
        while j < b.len() {
            match b[j] {
                b'\\' => j += 2,
                b'`' => {
                    push(out, i..j + 1, Tok::String);
                    self.pop();
                    self.after_value = true;
                    return j + 1;
                }
                b'$' if b.get(j + 1) == Some(&b'{') => {
                    if j > i {
                        push(out, i..j, Tok::String);
                    }
                    push(out, j..j + 2, Tok::Operator);
                    self.stack.push(Mode::Script(Script {
                        ts,
                        jsx,
                        until: Until::Brace,
                        depth: 0,
                        interp: true,
                    }));
                    self.after_value = false;
                    return j + 2;
                }
                _ => j += 1,
            }
        }
        push(out, i..text.len(), Tok::String);
        text.len()
    }

    /// Inside `<name …>`, JSX's or HTML's.
    fn tag(&mut self, text: &str, i: usize, out: &mut Spans) -> usize {
        let rest = &text[i..];
        let c = rest.chars().next().unwrap();
        let ts = self.ts;
        let Some(Mode::Tag(t)) = self.stack.last_mut() else {
            unreachable!()
        };
        if t.name.is_none() {
            if c == '/' && !t.closing {
                t.closing = true;
                return i + 1;
            }
            let n = rest
                .find(|ch: char| {
                    !(ch.is_alphanumeric() || matches!(ch, '-' | ':' | '.' | '_' | '!'))
                })
                .unwrap_or(rest.len());
            t.name = Some(rest[..n].to_ascii_lowercase());
            if n > 0 {
                push(out, i..i + n, Tok::Tag);
            }
            return i + n;
        }
        if c.is_whitespace() {
            return i + c.len_utf8();
        }
        if rest.starts_with("/>") {
            self.pop();
            self.after_value = true;
            return i + 2;
        }
        if c == '>' {
            if let Some(Mode::Tag(t)) = self.pop() {
                self.opened(t);
            }
            return i + 1;
        }
        if c == '"' || c == '\'' {
            let stop = string_end(text, i, c);
            push(out, i..stop, Tok::String);
            if stop - i < 2 || !text[..stop].ends_with(c) {
                self.stack.push(Mode::AttrString(c));
            } else if t.attr == "lang" || t.attr == "type" {
                t.lang = text[i + 1..stop - 1].to_ascii_lowercase();
            }
            return stop;
        }
        if c == '{' && t.jsx {
            self.stack.push(Mode::Script(Script {
                ts,
                jsx: true,
                until: Until::Brace,
                depth: 0,
                interp: false,
            }));
            return i + 1;
        }
        if c == '=' {
            push(out, i..i + 1, Tok::Operator);
            return i + 1;
        }
        // `class`, `v-on:click.prevent`, `@click`, `:value`, `[prop]`, `(click)`, `*ngIf`.
        let n = rest
            .find(|ch: char| !(ch.is_alphanumeric() || "-_:@.#[]()*$".contains(ch)))
            .unwrap_or(rest.len());
        if n > 0 {
            t.attr = rest[..n].to_ascii_lowercase();
            push(out, i..i + n, Tok::Attribute);
            return i + n;
        }
        i + c.len_utf8()
    }

    /// The `>` of tag `t`: into its children, or its script or style.
    fn opened(&mut self, t: Tag) {
        let name = t.name.unwrap_or_default();
        if t.jsx {
            if !t.closing {
                self.stack.push(Mode::Text { jsx: true });
            } else if matches!(self.stack.last(), Some(Mode::Text { jsx: true })) {
                self.pop();
            }
            self.after_value = true;
            return;
        }
        if t.closing {
            return;
        }
        let lang = t.lang.as_str();
        if name == "script" && !lang.contains("template") && !lang.contains("html") {
            self.stack.push(Mode::Script(Script {
                ts: matches!(lang, "ts" | "typescript" | "tsx"),
                jsx: matches!(lang, "tsx" | "jsx"),
                until: Until::Close("</script"),
                depth: 0,
                interp: false,
            }));
            self.after_value = false;
        } else if name == "style" {
            self.stack.push(Mode::Css(Css {
                scss: matches!(lang, "scss" | "sass" | "less"),
                until: Until::Close("</style"),
                depth: 0,
                stmt: None,
            }));
        }
    }

    /// JSX's children or HTML's text: only tags, `{…}` and entities.
    fn text(&mut self, text: &str, i: usize, out: &mut Spans, jsx: bool) -> usize {
        let rest = &text[i..];
        let c = rest.chars().next().unwrap();
        let after = &rest[c.len_utf8()..];
        if !jsx && rest.starts_with("<!--") {
            return self.block_comment(text, i, out, 4, "-->");
        }
        if c == '<'
            && after.starts_with(|n: char| n.is_ascii_alphabetic() || matches!(n, '/' | '>' | '!'))
        {
            self.stack.push(Mode::Tag(Tag {
                jsx,
                ..Tag::default()
            }));
            return i + 1;
        }
        if c == '{' && jsx {
            self.stack.push(Mode::Script(Script {
                ts: self.ts,
                jsx: true,
                until: Until::Brace,
                depth: 0,
                interp: false,
            }));
            return i + 1;
        }
        // `&amp;`, `&#39;`.
        if c == '&'
            && let Some(n) = after.find(';')
            && (1..=10).contains(&n)
            && after[..n]
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '#')
        {
            push(out, i..i + n + 2, Tok::Number);
            return i + n + 2;
        }
        i + c.len_utf8() + after.find(['<', '{', '&']).unwrap_or(after.len())
    }

    fn css(&mut self, text: &str, i: usize, out: &mut Spans, s: Css) -> usize {
        let rest = &text[i..];
        let c = rest.chars().next().unwrap();
        if let Until::Close(tag) = s.until
            && starts_ci(rest, tag)
        {
            self.pop();
            return i;
        }
        if c.is_whitespace() {
            return i + c.len_utf8();
        }
        if rest.starts_with("/*") {
            return self.block_comment(text, i, out, 2, "*/");
        }
        if s.scss && rest.starts_with("//") {
            push(out, i..text.len(), Tok::Comment);
            return text.len();
        }
        // SCSS's `#{$x}`.
        if rest.starts_with("#{") {
            let stop = rest.find('}').map_or(text.len(), |e| i + e + 1);
            push(out, i..stop, Tok::Variable);
            return stop;
        }
        let set = |this: &mut Self, f: &dyn Fn(&mut Css)| {
            if let Some(Mode::Css(s)) = this.stack.last_mut() {
                f(s);
            }
        };
        match c {
            '{' => {
                set(self, &|s| {
                    s.depth += 1;
                    s.stmt = None;
                });
                return i + 1;
            }
            '}' => {
                set(self, &|s| {
                    s.depth = s.depth.saturating_sub(1);
                    s.stmt = None;
                });
                return i + 1;
            }
            ';' => {
                set(self, &|s| s.stmt = None);
                return i + 1;
            }
            _ => {}
        }
        let Some(stmt) = s.stmt else {
            // A new statement: a selector, a declaration or an at-rule?
            let end = rest.find(['{', ';', '}']);
            let seg = &rest[..end.unwrap_or(rest.len())];
            let term = end.map(|e| rest.as_bytes()[e]);
            let var = c == '$' || c == '@' || rest.starts_with("--");
            if (s.depth > 0 || var) && is_decl(seg, term) {
                let n = 1 + rest[1..]
                    .find(|ch: char| !(ch.is_alphanumeric() || matches!(ch, '-' | '_')))
                    .unwrap_or(rest.len() - 1);
                push(out, i..i + n, if var { Tok::Variable } else { Tok::Type });
                let colon = i + n + text[i + n..].find(':').unwrap_or(0);
                set(self, &|s| s.stmt = Some(Stmt::Value));
                return colon + 1;
            }
            let stmt = if c == '@' {
                Stmt::At
            } else if s.depth == 0 || c.is_alphabetic() || ".#&:[*>+~%".contains(c) {
                Stmt::Selector
            } else {
                Stmt::Value
            };
            set(self, &|s| s.stmt = Some(stmt));
            return i;
        };
        match stmt {
            Stmt::Selector => css_selector(text, i, out),
            Stmt::Value => css_value(text, i, out, false),
            Stmt::At => css_value(text, i, out, true),
        }
    }

    fn json(&mut self, text: &str, i: usize, out: &mut Spans) -> usize {
        let rest = &text[i..];
        let c = rest.chars().next().unwrap();
        if c.is_whitespace() {
            return i + c.len_utf8();
        }
        if rest.starts_with("//") {
            push(out, i..text.len(), Tok::Comment);
            return text.len();
        }
        if rest.starts_with("/*") {
            return self.block_comment(text, i, out, 2, "*/");
        }
        if c == '"' || c == '\'' {
            let stop = string_end(text, i, c);
            let key = text[stop..].trim_start().starts_with(':');
            push(out, i..stop, if key { Tok::Tag } else { Tok::String });
            return stop;
        }
        if c.is_ascii_digit() || c == '-' {
            let n = 1 + rest[1..]
                .find(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.')))
                .unwrap_or(rest.len() - 1);
            push(out, i..i + n, Tok::Number);
            return i + n;
        }
        if ident_start(c) {
            let n = ident_len(rest);
            let word = &rest[..n];
            if matches!(word, "true" | "false" | "null") {
                push(out, i..i + n, Tok::Keyword);
            } else if rest[n..].trim_start().starts_with(':') {
                // JSON5's unquoted key.
                push(out, i..i + n, Tok::Tag);
            }
            return i + n;
        }
        i + c.len_utf8()
    }
}

/// A CSS selector's token: `div`, `.class`, `#id`, `:hover`, `[attr]`.
fn css_selector(text: &str, i: usize, out: &mut Spans) -> usize {
    let rest = &text[i..];
    let c = rest.chars().next().unwrap();
    match c {
        '.' | '#' | '%' => {
            let n = css_ident(&rest[1..]);
            if n > 0 {
                push(out, i..i + 1 + n, Tok::Attribute);
                return i + 1 + n;
            }
        }
        ':' => {
            let colons = if rest.starts_with("::") { 2 } else { 1 };
            let n = css_ident(&rest[colons..]);
            if n > 0 {
                push(out, i..i + colons + n, Tok::Attribute);
                return i + colons + n;
            }
        }
        '[' => {
            let n = css_ident(&rest[1..]);
            if n > 0 {
                push(out, i + 1..i + 1 + n, Tok::Attribute);
                return i + 1 + n;
            }
        }
        '"' | '\'' => {
            let stop = string_end(text, i, c);
            push(out, i..stop, Tok::String);
            return stop;
        }
        '&' | '>' | '+' | '~' | '*' => {
            push(out, i..i + 1, Tok::Operator);
            return i + 1;
        }
        '$' => {
            let n = 1 + css_ident(&rest[1..]);
            push(out, i..i + n, Tok::Variable);
            return i + n;
        }
        _ => {}
    }
    if c.is_ascii_digit() {
        let n = rest
            .find(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '.' | '%')))
            .unwrap_or(rest.len());
        push(out, i..i + n, Tok::Number);
        return i + n;
    }
    let n = css_ident(rest);
    if n > 0 {
        push(out, i..i + n, Tok::Tag);
        return i + n;
    }
    i + c.len_utf8()
}

/// A token of a declaration's value, or of an at-rule (`at`).
fn css_value(text: &str, i: usize, out: &mut Spans, at: bool) -> usize {
    let rest = &text[i..];
    let c = rest.chars().next().unwrap();
    let after = &rest[c.len_utf8()..];
    match c {
        '"' | '\'' => {
            let stop = string_end(text, i, c);
            push(out, i..stop, Tok::String);
            return stop;
        }
        // A color: `#fff`.
        '#' => {
            let n = after
                .find(|ch: char| !ch.is_ascii_alphanumeric())
                .unwrap_or(after.len());
            if n > 0 {
                push(out, i..i + 1 + n, Tok::Number);
                return i + 1 + n;
            }
        }
        // `!important`, `!default`.
        '!' => {
            let n = css_ident(after);
            if n > 0 {
                push(out, i..i + 1 + n, Tok::Keyword);
                return i + 1 + n;
            }
        }
        // `@media`; Less's `@color`; SCSS's `$color`.
        '@' | '$' => {
            let n = css_ident(after);
            if n > 0 {
                let tok = if c == '@' && at {
                    Tok::Keyword
                } else {
                    Tok::Variable
                };
                push(out, i..i + 1 + n, tok);
                return i + 1 + n;
            }
        }
        _ => {}
    }
    if rest.starts_with("--") {
        let n = css_ident(rest);
        push(out, i..i + n, Tok::Variable);
        return i + n;
    }
    if c.is_ascii_digit()
        || (matches!(c, '.' | '-' | '+')
            && after.starts_with(|n: char| n.is_ascii_digit() || n == '.'))
    {
        let n = c.len_utf8()
            + after
                .find(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '.' | '%')))
                .unwrap_or(after.len());
        push(out, i..i + n, Tok::Number);
        return i + n;
    }
    let n = css_ident(rest);
    if n > 0 {
        let word = &rest[..n];
        let next = &rest[n..];
        if let Some(args) = next.strip_prefix('(') {
            push(out, i..i + n, Tok::Function);
            // `url(a.png)`: its unquoted address is a string.
            if word.eq_ignore_ascii_case("url") && !args.trim_start().starts_with(['"', '\'']) {
                let close = next.find(')').map_or(text.len(), |p| i + n + p);
                if close > i + n + 1 {
                    push(out, i + n + 1..close, Tok::String);
                }
                return close;
            }
        } else if at && next.trim_start().starts_with(':') {
            // `(max-width: 600px)`
            push(out, i..i + n, Tok::Type);
        } else if at && matches!(word, "and" | "not" | "only" | "or") {
            push(out, i..i + n, Tok::Keyword);
        }
        return i + n;
    }
    i + c.len_utf8()
}

/// `color: red`, `$gap: 4px`, `box-shadow: 0 1px red,` not `a:hover {`
/// or `a:hover,` (a selector list going on to the next line).
fn is_decl(seg: &str, term: Option<u8>) -> bool {
    if term == Some(b'{') {
        return false;
    }
    let s = seg.strip_prefix(['$', '@']).unwrap_or(seg);
    let n = css_ident(s);
    let after = s[n..].trim_start();
    let Some(value) = after.strip_prefix(':') else {
        return false;
    };
    n > 0
        && !value.starts_with(':')
        && (value.starts_with(char::is_whitespace) || !seg.trim_end().ends_with(','))
}

fn css_ident(s: &str) -> usize {
    let start = s.starts_with(|c: char| c.is_alphabetic() || c == '_')
        || (s.starts_with('-')
            && s[1..].starts_with(|c: char| c.is_alphabetic() || c == '-' || c == '_'));
    if !start {
        return 0;
    }
    s.find(|c: char| !(c.is_alphanumeric() || matches!(c, '-' | '_')))
        .unwrap_or(s.len())
}

fn ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_' || c == '$'
}

fn ident_len(s: &str) -> usize {
    s.find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
        .unwrap_or(s.len())
}

/// The name that ends `before`, if it ends with one.
fn last_word(before: &str) -> &str {
    let start = before
        .char_indices()
        .rev()
        .find(|(_, c)| !(c.is_alphanumeric() || *c == '_' || *c == '$'))
        .map_or(0, |(p, c)| p + c.len_utf8());
    &before[start..]
}

/// A soft keyword used as one: followed by a name, a string, `{`, `[` or `*`
/// (`from 'x'`, `as const`, `type X`), not by `=`, `:` or `(`.
fn soft_keyword(after: &str) -> bool {
    after.starts_with(char::is_whitespace)
        && after.trim_start().starts_with(|c: char| {
            ident_start(c) || matches!(c, '"' | '\'' | '`' | '{' | '[' | '*')
        })
}

/// After a name, a function is assigned to it: `= (a) =>`, `= async x =>`,
/// `: function`.
fn arrow_def(after: &str) -> bool {
    let a = after.trim_start();
    let b = if let Some(r) = a.strip_prefix('=') {
        if r.starts_with(['=', '>']) {
            return false;
        }
        r
    } else if let Some(r) = a.strip_prefix(':') {
        if r.starts_with(':') {
            return false;
        }
        r
    } else {
        return false;
    };
    let mut b = b.trim_start();
    if let Some(r) = b.strip_prefix("async")
        && r.starts_with(|c: char| c.is_whitespace() || c == '(')
    {
        b = r.trim_start();
    }
    if let Some(r) = b.strip_prefix("function") {
        return !r.starts_with(|c: char| c.is_alphanumeric() || c == '_' || c == '$');
    }
    if b.starts_with('(') {
        let mut depth = 0;
        for (j, c) in b.char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        let t = b[j + 1..].trim_start();
                        return t.starts_with("=>") || (t.starts_with(':') && t.contains("=>"));
                    }
                }
                _ => {}
            }
        }
        return false;
    }
    let n = ident_len(b);
    n > 0 && b[n..].trim_start().starts_with("=>")
}

/// Where a regex literal starting at `i` ends, if it's one.
fn regex_end(text: &str, i: usize) -> Option<usize> {
    let b = text.as_bytes();
    if matches!(b.get(i + 1), None | Some(b'/' | b'*' | b' ')) {
        return None;
    }
    let mut j = i + 1;
    let mut class = false;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 1,
            b'[' => class = true,
            b']' => class = false,
            b'/' if !class => {
                j += 1;
                while j < b.len() && b[j].is_ascii_alphabetic() {
                    j += 1;
                }
                return Some(j);
            }
            _ => {}
        }
        j += 1;
    }
    None
}

fn starts_ci(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len() && s.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

#[cfg(test)]
mod tests {
    use crate::highlight::{Highlighter, Tok, Tok::*};

    /// Each line's tokens, lexed in order.
    fn lines(path: &str, text: &[&'static str]) -> Vec<Vec<(&'static str, Tok)>> {
        let mut h = Highlighter::new(path);
        text.iter()
            .map(|l| h.line(l).into_iter().map(|(r, t)| (&l[r], t)).collect())
            .collect()
    }

    fn toks(path: &str, line: &'static str) -> Vec<(&'static str, Tok)> {
        lines(path, &[line]).remove(0)
    }

    #[test]
    fn typescript() {
        assert_eq!(
            toks("a.ts", "import { x } from './x';"),
            [("import", Keyword), ("from", Keyword), ("'./x'", String)]
        );
        assert_eq!(
            toks(
                "a.ts",
                "export const load = async (id: string): Promise<User> => {"
            ),
            [
                ("export", Keyword),
                ("const", Keyword),
                ("load", Function),
                ("=", Operator),
                ("async", Keyword),
                ("string", Keyword),
                ("Promise", Type),
                ("<", Operator),
                ("User", Type),
                (">", Operator),
                ("=>", Operator),
            ]
        );
        assert_eq!(
            toks("a.ts", "type Props = { readonly id: number };"),
            [
                ("type", Keyword),
                ("Props", Type),
                ("=", Operator),
                ("readonly", Keyword),
                ("number", Keyword)
            ]
        );
        assert_eq!(
            toks("a.ts", "@Component({ selector: 'x' })"),
            [("@Component", Type), ("'x'", String)]
        );
        // Soft keywords as names, members named like keywords.
        assert_eq!(
            toks("a.ts", "const type = map.get(from);"),
            [("const", Keyword), ("=", Operator), ("get", Function)]
        );
        assert_eq!(
            toks("a.ts", "class A extends B implements C {}"),
            [
                ("class", Keyword),
                ("A", Type),
                ("extends", Keyword),
                ("B", Type),
                ("implements", Keyword),
                ("C", Type)
            ]
        );
        // `<` compares in .ts: no JSX.
        assert_eq!(
            toks("a.ts", "if (a <b) {}"),
            [("if", Keyword), ("<", Operator)]
        );
    }

    #[test]
    fn javascript_templates_and_regexes() {
        assert_eq!(
            toks("a.js", "const s = `id ${user.id} ok`;"),
            [
                ("const", Keyword),
                ("=", Operator),
                ("`id ", String),
                ("${", Operator),
                ("}", Operator),
                (" ok`", String)
            ]
        );
        // A template over lines, with `${…}` holding braces.
        assert_eq!(
            lines("a.js", &["sql`", "  select ${f({ a: 1 })}", "`;", "x"]),
            [
                vec![("sql", Function), ("`", String)],
                vec![
                    ("  select ", String),
                    ("${", Operator),
                    ("f", Function),
                    ("1", Number),
                    ("}", Operator)
                ],
                vec![("`", String)],
                vec![],
            ]
        );
        assert_eq!(
            toks("a.js", "const re = /a\\/[/]b/gi; x = a / b / c;"),
            [
                ("const", Keyword),
                ("=", Operator),
                ("/a\\/[/]b/gi", String),
                ("=", Operator),
                ("/", Operator),
                ("/", Operator)
            ]
        );
        assert_eq!(
            toks("a.mjs", "#!/usr/bin/env node"),
            [("#!/usr/bin/env node", Comment)]
        );
        assert_eq!(
            toks("a.js", "items.map(x => x * 2)"),
            [
                ("map", Function),
                ("=>", Operator),
                ("*", Operator),
                ("2", Number)
            ]
        );
    }

    #[test]
    fn jsx() {
        assert_eq!(
            toks(
                "a.tsx",
                "return <Button onClick={() => go(1)} disabled>Don't {name}</Button>;"
            ),
            [
                ("return", Keyword),
                ("Button", Tag),
                ("onClick", Attribute),
                ("=", Operator),
                ("=>", Operator),
                ("go", Function),
                ("1", Number),
                ("disabled", Attribute),
                ("Button", Tag),
            ]
        );
        // An element over lines; the script after it.
        assert_eq!(
            lines(
                "a.jsx",
                &[
                    "  <div",
                    "    className=\"a\"",
                    "  >",
                    "    it's <b>x</b>",
                    "  </div>",
                    ");",
                    "const a = 1;"
                ]
            ),
            [
                vec![("div", Tag)],
                vec![("className", Attribute), ("=", Operator), ("\"a\"", String)],
                vec![],
                vec![("b", Tag), ("b", Tag)],
                vec![("div", Tag)],
                vec![],
                vec![("const", Keyword), ("=", Operator), ("1", Number)],
            ]
        );
        // Generics aren't elements.
        assert_eq!(
            toks("a.tsx", "useState<string>(x)"),
            [
                ("useState", Function),
                ("<", Operator),
                ("string", Keyword),
                (">", Operator)
            ]
        );
    }

    #[test]
    fn css_and_scss() {
        assert_eq!(
            toks("a.css", ".btn:hover > a, #main {"),
            [
                (".btn:hover", Attribute),
                (">", Operator),
                ("a", Tag),
                ("#main", Attribute)
            ]
        );
        assert_eq!(
            lines(
                "a.css",
                &[
                    "a {",
                    "  color: #fff !important;",
                    "  margin: 0 -4px;",
                    "  background: url(a.png) rgba(0, 0, 0, .5);",
                    "}"
                ]
            ),
            [
                vec![("a", Tag)],
                vec![("color", Type), ("#fff", Number), ("!important", Keyword)],
                vec![("margin", Type), ("0", Number), ("-4px", Number)],
                vec![
                    ("background", Type),
                    ("url", Function),
                    ("a.png", String),
                    ("rgba", Function),
                    ("0", Number),
                    ("0", Number),
                    ("0", Number),
                    (".5", Number)
                ],
                vec![],
            ]
        );
        assert_eq!(
            toks("a.css", "@media screen and (max-width: 600px) {"),
            [
                ("@media", Keyword),
                ("and", Keyword),
                ("max-width", Type),
                ("600px", Number)
            ]
        );
        assert_eq!(
            lines(
                "a.scss",
                &[
                    "$gap: 4px; // half",
                    ".a {",
                    "  &:hover { gap: $gap; }",
                    "  @include m(1);",
                    "}"
                ]
            ),
            [
                vec![("$gap", Variable), ("4px", Number), ("// half", Comment)],
                vec![(".a", Attribute)],
                vec![
                    ("&", Operator),
                    (":hover", Attribute),
                    ("gap", Type),
                    ("$gap", Variable)
                ],
                vec![("@include", Keyword), ("m", Function), ("1", Number)],
                vec![],
            ]
        );
        assert_eq!(
            toks("a.css", ":root { --gap: var(--x); }"),
            [
                (":root", Attribute),
                ("--gap", Variable),
                ("var", Function),
                ("--x", Variable)
            ]
        );
        // A value over lines.
        assert_eq!(
            lines(
                "a.css",
                &[
                    "a {",
                    "  box-shadow: 0 1px red,",
                    "    inset 0 2px blue;",
                    "}"
                ]
            )[2],
            [("0", Number), ("2px", Number)]
        );
    }

    #[test]
    fn json() {
        assert_eq!(
            toks(
                "package.json",
                r#"  "version": "1.0", "n": -1.5e3, "ok": true, "x": null"#
            ),
            [
                ("\"version\"", Tag),
                ("\"1.0\"", String),
                ("\"n\"", Tag),
                ("-1.5e3", Number),
                ("\"ok\"", Tag),
                ("true", Keyword),
                ("\"x\"", Tag),
                ("null", Keyword)
            ]
        );
        assert_eq!(toks("tsconfig.jsonc", "// c"), [("// c", Comment)]);
        assert_eq!(
            toks("composer.lock", "{\"a\": 1}"),
            [("\"a\"", Tag), ("1", Number)]
        );
    }

    #[test]
    fn html_and_vue() {
        assert_eq!(
            toks(
                "a.html",
                "<a href=\"/x\" class='b'>Tom &amp; Jerry</a> <!-- c -->"
            ),
            [
                ("a", Tag),
                ("href", Attribute),
                ("=", Operator),
                ("\"/x\"", String),
                ("class", Attribute),
                ("=", Operator),
                ("'b'", String),
                ("&amp;", Number),
                ("a", Tag),
                ("<!-- c -->", Comment)
            ]
        );
        assert_eq!(
            lines(
                "A.vue",
                &[
                    "<template><p :class=\"x\">{{ n }}</p></template>",
                    "<script setup lang=\"ts\">",
                    "const n: number = 1",
                    "</script>",
                    "<style scoped lang=\"scss\">",
                    ".a { color: red }",
                    "</style>"
                ]
            ),
            [
                vec![
                    ("template", Tag),
                    ("p", Tag),
                    (":class", Attribute),
                    ("=", Operator),
                    ("\"x\"", String),
                    ("p", Tag),
                    ("template", Tag)
                ],
                vec![
                    ("script", Tag),
                    ("setup", Attribute),
                    ("lang", Attribute),
                    ("=", Operator),
                    ("\"ts\"", String)
                ],
                vec![
                    ("const", Keyword),
                    ("number", Keyword),
                    ("=", Operator),
                    ("1", Number)
                ],
                vec![("script", Tag)],
                vec![
                    ("style", Tag),
                    ("scoped", Attribute),
                    ("lang", Attribute),
                    ("=", Operator),
                    ("\"scss\"", String)
                ],
                vec![(".a", Attribute), ("color", Type)],
                vec![("style", Tag)],
            ]
        );
        // A script and its end on one line.
        assert_eq!(
            toks("a.html", "<script>let a = 1</script>")[..2],
            [("script", Tag), ("let", Keyword)]
        );
    }

    #[test]
    fn odd_lines_dont_panic() {
        let odd = [
            "",
            " ",
            "<",
            "</",
            "<>",
            "`",
            "${",
            "}",
            "{",
            "/",
            "/*",
            "*/",
            "'",
            "\"",
            "@",
            "#",
            "$",
            "-",
            "--",
            "!",
            "a: ",
            "<a b=\"",
            "url(",
            "ž ū <ž>",
            "x = /[/",
            "@media",
            "&",
            "#{",
            "<!--",
            ".",
            "1.",
            "-.",
            "a{b:c}",
        ];
        for path in [
            "a.ts", "a.tsx", "a.js", "a.css", "a.scss", "a.json", "a.html", "a.vue",
        ] {
            let mut h = Highlighter::new(path);
            for l in odd {
                for (r, _) in h.line(l) {
                    assert!(
                        l.get(r.clone()).is_some() && r.start <= r.end,
                        "{path} {l:?}"
                    );
                }
            }
        }
    }
}
