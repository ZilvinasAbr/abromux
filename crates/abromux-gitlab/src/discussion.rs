//! Notes and threads as GitLab draws them: avatar, name, @username, time,
//! the markdown body, and suggestions as a small diff ("Suggested change").

use crate::{
    diff::{Kind, Line, word_changes},
    highlight::Highlighter,
    style::*,
    time,
    types::{Discussion, Note, Suggestion, User},
    view::GitLabView,
};
use gpui::{prelude::*, *};
use gpui_component::{ActiveTheme as _, text::TextView};

/// Markdown split around what we draw ourselves.
enum Part<'a> {
    Markdown(String),
    Suggestion(&'a Suggestion),
    /// A fenced code block: its language (may be empty) and text.
    Code(String, String),
}

/// The body's markdown, with each ```` ```suggestion ```` block replaced by
/// GitLab's parsed suggestion of the same index, and other fenced code
/// blocks taken out to be highlighted as GitLab does.
fn parts<'a>(body: &str, suggestions: &'a [Suggestion]) -> Vec<Part<'a>> {
    let mut out = vec![];
    let mut text = String::new();
    let mut lines = body.lines();
    let mut index = 0;
    let flush = |text: &mut String, out: &mut Vec<Part<'a>>| {
        if !text.trim().is_empty() {
            out.push(Part::Markdown(text.trim().to_string()));
        }
        text.clear();
    };
    while let Some(l) = lines.next() {
        let fence = l.trim_start();
        if let Some(info) = fence.strip_prefix("```") {
            let info = info.trim().to_string();
            let mut code = String::new();
            for l in lines.by_ref() {
                if l.trim_start().starts_with("```") {
                    break;
                }
                code.push_str(l);
                code.push('\n');
            }
            flush(&mut text, &mut out);
            if info.starts_with("suggestion") {
                if let Some(s) = suggestions.get(index) {
                    out.push(Part::Suggestion(s));
                }
                index += 1;
            } else {
                let lang = info.split_whitespace().next().unwrap_or("").to_string();
                out.push(Part::Code(lang, code.trim_end_matches('\n').to_string()));
            }
            continue;
        }
        text.push_str(l);
        text.push('\n');
    }
    flush(&mut text, &mut out);
    out
}

/// A file name the highlighter knows for a code block's language.
fn lang_path(lang: &str) -> String {
    let ext = match lang.to_ascii_lowercase().as_str() {
        "rust" | "rs" => "rs",
        "javascript" | "js" | "jsx" => "js",
        "typescript" | "ts" | "tsx" => "ts",
        "python" | "py" => "py",
        "ruby" | "rb" => "rb",
        "go" | "golang" => "go",
        "shell" | "sh" | "bash" | "zsh" | "console" => "sh",
        "yaml" | "yml" => "yml",
        "json" => "json",
        "toml" => "toml",
        "c" | "cpp" | "c++" | "java" | "kotlin" | "swift" | "css" | "scss" | "php" => "c",
        "markdown" | "md" => "md",
        _ => "txt",
    };
    format!("x.{ext}")
}

/// One diff line of code: number cells, sign, and its text (drawn by the
/// caller: selectable, select.rs); `selected` as GitLab draws the line you
/// clicked the number of.
pub fn code_line_with(l: &Line, mono: &SharedString, gutter_extra: Option<AnyElement>, selected: bool, text: AnyElement) -> Div {
    let (bg, num_color, sign) = match l.kind {
        Kind::Added => (c(ADD_BG), c(ADD_NUM), "+"),
        Kind::Removed => (c(DEL_BG), c(DEL_NUM), "-"),
        _ => (c(CODE_BG), ca(LINE_NUM), " "),
    };
    let (bg, num_bg, num_color) = if selected {
        (c(SELECTED_BG), Some(c(SELECTED_NUM_BG)), c(TEXT))
    } else {
        (bg, None, num_color)
    };
    let num = |n: Option<u32>| {
        div()
            .w(px(NUM_W))
            .flex_none()
            .pl(px(5.))
            .pr(px(10.))
            .flex()
            .justify_end()
            .text_color(num_color)
            .when_some(num_bg, |d, b| d.bg(b))
            .when(selected, |d| d.underline())
            .children(n.map(|n| n.to_string()))
    };
    code(div(), mono.clone())
        .flex()
        .w_full()
        .bg(bg)
        .child(div().relative().flex().flex_none().child(num(l.old)).child(num(l.new)).children(gutter_extra))
        .child(div().w(px(18.75)).flex_none().flex().justify_center().text_color(num_color).child(sign))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .pr(px(18.75))
                .text_color(if l.kind == Kind::NoNewline { ca(LINE_NUM) } else { c(CODE_TEXT) })
                .child(text),
        )
}

/// One side of a side-by-side row: the line's number on that side, sign and
/// text, or an empty cell when the other side has a line and this one doesn't.
pub fn half_with(l: Option<&Line>, new_side: bool, text: Option<AnyElement>) -> Div {
    let (Some(l), Some(text)) = (l, text) else {
        return div().flex_1().min_w(px(0.)).bg(c(EMPTY_BG));
    };
    let (bg, num_color, sign) = match l.kind {
        Kind::Added => (c(ADD_BG), c(ADD_NUM), "+"),
        Kind::Removed => (c(DEL_BG), c(DEL_NUM), "-"),
        _ => (c(CODE_BG), ca(LINE_NUM), " "),
    };
    let n = if new_side { l.new } else { l.old };
    div()
        .flex_1()
        .min_w(px(0.))
        .flex()
        .bg(bg)
        .child(
            div()
                .w(px(NUM_W))
                .flex_none()
                .pl(px(5.))
                .pr(px(10.))
                .flex()
                .justify_end()
                .text_color(num_color)
                .children(n.map(|n| n.to_string())),
        )
        .child(div().w(px(18.75)).flex_none().flex().justify_center().text_color(num_color).child(sign))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .pr(px(10.))
                .text_color(if l.kind == Kind::NoNewline { ca(LINE_NUM) } else { c(CODE_TEXT) })
                .child(text),
        )
}

/// A line's text and its highlights: syntax colors and changed words.
pub fn line_highlights(l: &Line) -> (SharedString, Vec<(std::ops::Range<usize>, HighlightStyle)>) {
    if l.kind == Kind::NoNewline {
        return (SharedString::from(l.text.clone()), vec![]);
    }
    let mut highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = l
        .spans
        .iter()
        .map(|(r, t)| (r.clone(), HighlightStyle { color: Some(token(*t)), ..Default::default() }))
        .collect();
    let word = if l.kind == Kind::Added { ca(ADD_WORD) } else { ca(DEL_WORD) };
    for r in &l.changed {
        highlights = overlay(highlights, r.clone(), word);
    }
    (SharedString::from(l.text.clone()), highlights)
}

/// Add a background over `range` to syntax highlights, splitting the ones
/// it crosses.
pub(crate) fn overlay(
    hl: Vec<(std::ops::Range<usize>, HighlightStyle)>,
    range: std::ops::Range<usize>,
    bg: Hsla,
) -> Vec<(std::ops::Range<usize>, HighlightStyle)> {
    // Cut at every span edge and the range's, then style each piece.
    let mut cuts: Vec<usize> = hl
        .iter()
        .flat_map(|(r, _)| [r.start, r.end])
        .chain([range.start, range.end])
        .collect();
    cuts.sort_unstable();
    cuts.dedup();
    let mut out = vec![];
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let base = hl.iter().find(|(r, _)| r.start <= a && b <= r.end).map(|(_, s)| *s);
        let inside = range.start <= a && b <= range.end;
        match (base, inside) {
            (_, true) => out.push((
                a..b,
                HighlightStyle { background_color: Some(bg), ..base.unwrap_or_default() },
            )),
            (Some(s), false) => out.push((a..b, s)),
            (None, false) => {}
        }
    }
    out
}

impl GitLabView {
    pub(crate) fn mono(&self, cx: &App) -> SharedString {
        cx.theme().mono_font_family.clone()
    }

    pub(crate) fn note_header(&self, author: &User, created: &str, mr_author: u64, actions: Option<Div>) -> Div {
        // In a side-by-side diff's half-width thread GitLab leaves out the
        // @username and the badges, keeping the header one line.
        let compact = self.loc.parallel && self.loc.section == crate::Section::Changes;
        // A local diff's authors have no roles in it.
        let local = self.is_local();
        // Who and when wrap within their part; the badges and buttons keep
        // to the right of the first line.
        let who = div()
            .flex_1()
            .min_w(px(0.))
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1()
            .child(div().font_weight(FontWeight::SEMIBOLD).text_color(c(TEXT)).child(author.name.clone()))
            .when(!compact, |d| d.child(div().text_color(c(MUTED)).child(format!("@{}", author.username))))
            .child(when(created).ml_1().text_color(c(MUTED)));
        div()
            .flex()
            .items_start()
            .gap_1()
            .child(who)
            .when(!compact && !local && author.id == mr_author, |d| d.child(badge("Author", c(0xa4a3a8), c(BG)).flex_none()))
            .when_some(self.review.ready().and_then(|r| r.roles.get(&author.id).copied()).filter(|_| !compact && !local), |d, role| {
                d.child(badge(role, c(0xa4a3a8), c(BG)).flex_none())
            })
            .children(actions.map(|a| a.flex_none().mt(px(-4.))))
    }

    /// GitLab's buttons on a thread's first note: resolve, reply, copy link.
    fn thread_actions(&self, d: &Discussion, first: &Note, cx: &mut Context<Self>) -> Div {
        let button = |id: String, name: &'static str, color: Hsla, tip: &'static str| {
            div()
                .id(SharedString::from(id))
                .size(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .hover(|s| s.bg(ca(0x89888d66)))
                .child(icon(name, 16., color))
                .tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(tip).build(window, cx))
        };
        let writes = self.me.is_some();
        let resolved = d.resolved();
        let link = format!("{}#note_{}", self.review.ready().map_or("", |r| r.mr.web_url.as_str()), first.id);
        div()
            .flex()
            .items_center()
            .ml_1()
            .when(writes && d.resolvable(), |el| {
                let id = d.id.clone();
                el.child(
                    button(
                        format!("note-resolve-{}", d.id),
                        if resolved { "check-circle-filled" } else { "check-circle" },
                        if resolved { c(GREEN) } else { c(MUTED) },
                        if resolved { "Unresolve thread" } else { "Resolve thread" },
                    )
                    .on_click(cx.listener(move |v, _, window, cx| v.set_resolved(&id, !resolved, window, cx))),
                )
            })
            .when(writes, |el| {
                let id = d.id.clone();
                el.child(
                    button(format!("note-reply-{}", d.id), "reply", c(MUTED), "Reply to comment")
                        .on_click(cx.listener(move |v, _, window, cx| v.open_reply(&id, window, cx))),
                )
            })
            .when(self.has_web(), |el| {
                el.child(
                    button(format!("note-link-{}", d.id), "link", c(MUTED), "Copy link")
                        .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(link.clone()))),
                )
            })
    }

    /// A note: avatar (unless the timeline shows it already), header, body.
    pub(crate) fn note(
        &self,
        n: &Note,
        path: Option<&str>,
        avatar: bool,
        actions: Option<Div>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let mr_author = self.mr_author_id();
        let body = self.markdown(("note", n.id as usize), &n.body, &n.suggestions, path, window, cx);
        div()
            .flex()
            .gap_2()
            .px_4()
            .py_3()
            .when(avatar, |d| d.child(self.avatars.of(&n.author, 24.)))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(self.note_header(&n.author, &n.created_at, mr_author, actions))
                    .child(body),
            )
    }

    /// GitLab-flavored markdown: the text by gpui-component's renderer, and
    /// suggestions and code blocks drawn as GitLab draws them.
    pub(crate) fn markdown(
        &self,
        id: (&'static str, usize),
        body: &str,
        suggestions: &[Suggestion],
        path: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut out = div().flex().flex_col().gap_2().text_color(c(TEXT));
        let web = self.review.ready().map(|r| r.mr.web_url.clone()).unwrap_or_default();
        let project = web.split("/-/").next().unwrap_or_default().to_string();
        let host = self.host();
        for (i, p) in parts(body, suggestions).into_iter().enumerate() {
            out = match p {
                Part::Markdown(md) => out.child(
                    TextView::markdown((id.0, id.1 * 64 + i), link_references(&crate::emoji::expand(&md), &project, &host), window, cx)
                        .style(md_style())
                        .selectable(true)
                        .text_size(px(14.)),
                ),
                Part::Suggestion(s) => out.child(self.suggestion(s, path, cx)),
                Part::Code(lang, code) => {
                    let key = SharedString::from(format!("copy-code-{}-{}", id.0, id.1 * 64 + i));
                    out.child(self.code_block(&lang, &code, key, cx))
                }
            };
        }
        out
    }

    /// A fenced code block as GitLab draws it: the code background, a
    /// border, 12px padding, highlighted 14px mono on 22.4px lines.
    /// A fenced code block, highlighted, with GitLab's copy button at its
    /// top right; `key` names the button.
    fn code_block(&self, lang: &str, code: &str, key: SharedString, cx: &mut Context<Self>) -> Div {
        let mut hl = Highlighter::new(&lang_path(lang));
        let group = crate::select::Group::block(&key);
        let lines: Vec<AnyElement> = code.lines().enumerate().map(|(i, l)| {
            let highlights: Vec<_> = hl
                .line(l)
                .into_iter()
                .map(|(r, t)| (r, HighlightStyle { color: Some(token(t)), ..Default::default() }))
                .collect();
            // An empty line still takes its height.
            let text = if l.is_empty() { " ".to_string() } else { l.to_string() };
            self.sel_text(SharedString::from(text), highlights, group, i, cx)
        }).collect();
        let mut block = div()
            .p(px(12.))
            .rounded(px(4.))
            .border_1()
            .border_color(c(BORDER))
            .bg(c(CODE_BG))
            .text_color(c(CODE_TEXT))
            .font_family(self.mono(cx))
            .text_size(px(14.))
            .line_height(px(22.4));
        block.text_style().get_or_insert_with(Default::default).font_features = Some(FontFeatures::disable_ligatures());
        let copy = code.to_string();
        div().relative().child(block.flex().flex_col().children(lines)).child(
            div()
                .id(key)
                .absolute()
                .top(px(8.))
                .right(px(8.))
                .size(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .bg(ca(0x89888d66))
                .cursor_pointer()
                .hover(|s| s.bg(ca(0x89888d99)))
                .child(icon("copy", 14., c(TEXT)))
                .tooltip(hint("Copy to clipboard"))
                .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))),
        )
    }

    /// GitLab's "Suggested change": the lines it replaces and with what.
    fn suggestion(&self, s: &Suggestion, path: Option<&str>, cx: &mut Context<Self>) -> Div {
        let mono = self.mono(cx);
        let mut hl = Highlighter::new(path.unwrap_or(""));
        let mut lines: Vec<Line> = vec![];
        for (i, t) in s.from_content.lines().enumerate() {
            lines.push(Line {
                kind: Kind::Removed,
                old: Some(s.from_line + i as u32),
                new: None,
                spans: hl.line(t),
                text: t.to_string(),
                changed: vec![],
            });
        }
        let mut hl = Highlighter::new(path.unwrap_or(""));
        for (i, t) in s.to_content.lines().enumerate() {
            lines.push(Line {
                kind: Kind::Added,
                old: None,
                new: Some(s.from_line + i as u32),
                spans: hl.line(t),
                text: t.to_string(),
                changed: vec![],
            });
        }
        let removed = s.from_content.lines().count();
        if removed == s.to_content.lines().count() {
            for k in 0..removed {
                let (o, n) = word_changes(&lines[k].text, &lines[removed + k].text);
                lines[k].changed = o;
                lines[removed + k].changed = n;
            }
        }
        div()
            .flex()
            .flex_col()
            // overflow_hidden lets a flex item shrink to nothing; don't.
            .flex_none()
            .rounded(px(8.))
            .border_1()
            .border_color(c(BG))
            .overflow_hidden()
            .child(
                div()
                    .px_3()
                    .py_2()
                    .bg(c(RAISED))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c(TEXT))
                    .child(if s.applied { "Suggestion applied" } else { "Suggested change" }),
            )
            .children({
                let group = crate::select::Group::block(("suggestion", s.from_line, &s.to_content));
                let rows: Vec<Div> = lines
                    .iter()
                    .enumerate()
                    .map(|(i, l)| {
                        let (t, h) = line_highlights(l);
                        let text = self.sel_text(t, h, group, i, cx);
                        code_line_with(l, &mono, None, false, text)
                    })
                    .collect();
                rows
            })
    }

    /// The notes of a thread; with replies, a "Collapse replies" toggle after
    /// the first, as in GitLab's diffs (`toggle`; the overview has its own
    /// above). `first_avatar`: false where the timeline already shows the
    /// first note's author.
    pub(crate) fn thread_notes(
        &self,
        d: &Discussion,
        path: Option<&str>,
        first_avatar: bool,
        toggle: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let notes: Vec<&Note> = d.notes.iter().filter(|n| !n.system).collect();
        let mut out = vec![];
        let Some(first) = notes.first() else { return out };
        let actions = self.thread_actions(d, first, cx);
        out.push(self.note(first, path, first_avatar, Some(actions), window, cx).into_any_element());
        if !toggle {
            for n in &notes[1..] {
                out.push(self.note(n, path, true, None, window, cx).into_any_element());
            }
            return out;
        }
        if notes.len() > 1 {
            let collapsed = self.collapsed_replies.contains(&d.id);
            let id = d.id.clone();
            out.push(
                div()
                    .id(SharedString::from(format!("replies-{}", d.id)))
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_4()
                    .py_2()
                    .border_t_1()
                    .border_color(c(RAISED))
                    .cursor_pointer()
                    .text_color(c(LINK))
                    .child(app_icon(if collapsed { "chevron-right" } else { "chevron-down" }, 14., c(LINK)))
                    .when(collapsed, |el| {
                        let last = notes.last().unwrap();
                        el.child(
                            div()
                                .flex()
                                .gap_1()
                                .items_center()
                                .child(div().flex().children(notes[1..].iter().take(3).map(|n| self.avatars.of(&n.author, 20.))))
                                .child(format!("{} repl{}", notes.len() - 1, if notes.len() == 2 { "y" } else { "ies" }))
                                .child(
                                    div().text_color(c(MUTED)).child(format!(
                                        "Last reply by {} {}",
                                        last.author.name,
                                        time::ago(&last.created_at, time::now())
                                    )),
                                ),
                        )
                    })
                    .when(!collapsed, |el| el.child("Collapse replies"))
                    .on_click(cx.listener(move |v, _, _, cx| {
                        if !v.collapsed_replies.remove(&id) {
                            v.collapsed_replies.insert(id.clone());
                        }
                        cx.notify();
                    }))
                    .into_any_element(),
            );
            if !collapsed {
                for n in &notes[1..] {
                    out.push(self.note(n, path, true, None, window, cx).into_any_element());
                }
            }
        }
        out
    }
}

/// GitLab's references as links: `#12` the project's issue, `!3` its merge
/// request, `@bob` the user. Not in code (`…`), and only as a word of its
/// own, so `a#1`, URLs and headings stay as written.
fn link_references(md: &str, project: &str, host: &str) -> String {
    if project.is_empty() {
        return md.to_string();
    }
    let mut out = String::with_capacity(md.len());
    let mut in_code = false;
    let mut prev: Option<char> = None;
    let mut chars = md.char_indices().peekable();
    while let Some((i, ch)) = chars.next() {
        if ch == '`' {
            in_code = !in_code;
        }
        let starts_word = prev.is_none_or(|p| p.is_whitespace() || p == '(');
        if !in_code && starts_word && matches!(ch, '#' | '!' | '@') {
            let rest = &md[i + 1..];
            let len = match ch {
                '@' => rest.find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))).unwrap_or(rest.len()),
                _ => rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len()),
            };
            // A trailing dot ends the sentence, not the name.
            let word = rest[..len].trim_end_matches('.');
            if !word.is_empty() {
                let url = match ch {
                    '#' => format!("{project}/-/issues/{word}"),
                    '!' => format!("{project}/-/merge_requests/{word}"),
                    _ => format!("{host}/{word}"),
                };
                out.push_str(&format!("[{ch}{word}]({url})"));
                for _ in 0..word.chars().count() {
                    prev = chars.next().map(|(_, c)| c);
                }
                continue;
            }
        }
        out.push(ch);
        prev = Some(ch);
    }
    out
}

#[cfg(test)]
mod reference_tests {
    use super::link_references;

    #[test]
    fn references_become_links() {
        let p = "http://gl/a/b";
        let l = |s: &str| link_references(s, p, "http://gl");
        assert_eq!(l("Closes #12."), "Closes [#12](http://gl/a/b/-/issues/12).");
        assert_eq!(l("see !3 and @bob."), "see [!3](http://gl/a/b/-/merge_requests/3) and [@bob](http://gl/bob).");
        assert_eq!(l("(#4)"), "([#4](http://gl/a/b/-/issues/4))");
        // Left alone: code, mid-word, headings, a bang that isn't one.
        assert_eq!(l("`#12` a#1 # Title Wow! @"), "`#12` a#1 # Title Wow! @");
        assert_eq!(l("mail me at x@y.z"), "mail me at x@y.z");
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui's `test` macro would shadow `#[test]`.
    use super::{Part, overlay, parts};
    use crate::types::{Note, Suggestion};
    use gpui::{HighlightStyle, Hsla, rgb};

    #[test]
    fn splits_suggestions_out_of_bodies() {
        let n = Note {
            body: "Simpler:\n\n```suggestion:-0+0\n  x // y\n```\nafter".into(),
            suggestions: vec![Suggestion { from_line: 17, ..Default::default() }],
            ..Default::default()
        };
        let p = parts(&n.body, &n.suggestions);
        assert_eq!(p.len(), 3);
        assert!(matches!(&p[0], Part::Markdown(m) if m.trim() == "Simpler:"));
        assert!(matches!(&p[1], Part::Suggestion(s) if s.from_line == 17));
        assert!(matches!(&p[2], Part::Markdown(m) if m.trim() == "after"));
        // Other code blocks come out with their language.
        let p = parts("Use:\n```rust\nlet l = 3;\n\nl\n```\n", &[]);
        assert!(matches!(&p[1], Part::Code(lang, code) if lang == "rust" && code == "let l = 3;\n\nl"));
        assert_eq!(super::lang_path("Rust"), "x.rs");
    }

    #[test]
    fn overlays_a_background_across_spans() {
        let s = |c: u32| HighlightStyle { color: Some(rgb(c).into()), ..Default::default() };
        let bg: Hsla = rgb(0x00ff00).into();
        // "ab cd ef": spans on "ab" (0..2) and "ef" (6..8); changed 1..7.
        let out = overlay(vec![(0..2, s(1)), (6..8, s(2))], 1..7, bg);
        let ranges: Vec<_> = out.iter().map(|(r, h)| (r.clone(), h.background_color.is_some())).collect();
        assert_eq!(ranges, [(0..1, false), (1..2, true), (2..6, true), (6..7, true), (7..8, false)]);
    }
}
