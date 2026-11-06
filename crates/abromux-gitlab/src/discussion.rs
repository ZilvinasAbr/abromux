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

/// One diff line of a code block: number cells, sign, highlighted text.
pub fn code_line(l: &Line, mono: &SharedString, gutter_extra: Option<AnyElement>) -> Div {
    code_line_sel(l, mono, gutter_extra, false)
}

/// [`code_line`], drawn as GitLab draws the line you clicked the number of.
pub fn code_line_sel(l: &Line, mono: &SharedString, gutter_extra: Option<AnyElement>, selected: bool) -> Div {
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
    let text = styled_text(l);
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
pub fn half(l: Option<&Line>, new_side: bool) -> Div {
    let Some(l) = l else {
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
                .child(styled_text(l)),
        )
}

/// A line's text with its syntax colors and, where it changed, GitLab's
/// word highlight.
fn styled_text(l: &Line) -> StyledText {
    if l.kind == Kind::NoNewline {
        return StyledText::new(SharedString::from(l.text.clone()));
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
    StyledText::new(SharedString::from(l.text.clone())).with_highlights(highlights)
}

/// Add a background over `range` to syntax highlights, splitting the ones
/// it crosses.
fn overlay(
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

    pub(crate) fn note_header(&self, author: &User, created: &str, mr_author: u64) -> Div {
        div()
            .flex()
            .items_center()
            .gap_1()
            .flex_wrap()
            .child(div().font_weight(FontWeight::SEMIBOLD).text_color(c(TEXT)).child(author.name.clone()))
            .child(div().text_color(c(MUTED)).child(format!("@{}", author.username)))
            .child(when(created).ml_1().text_color(c(MUTED)))
            .child(div().flex_1())
            .when(author.id == mr_author, |d| d.child(badge("Author", c(0xa4a3a8), c(BG))))
            .when_some(self.review.ready().and_then(|r| r.roles.get(&author.id).copied()), |d, role| {
                d.child(badge(role, c(0xa4a3a8), c(BG)))
            })
    }

    /// A note: avatar (unless the timeline shows it already), header, body.
    pub(crate) fn note(
        &self,
        n: &Note,
        path: Option<&str>,
        avatar: bool,
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
                    .child(self.note_header(&n.author, &n.created_at, mr_author))
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
        for (i, p) in parts(body, suggestions).into_iter().enumerate() {
            out = match p {
                Part::Markdown(md) => out.child(
                    TextView::markdown((id.0, id.1 * 64 + i), crate::emoji::expand(&md), window, cx)
                        .style(md_style())
                        .selectable(true)
                        .text_size(px(14.)),
                ),
                Part::Suggestion(s) => out.child(self.suggestion(s, path, cx)),
                Part::Code(lang, code) => out.child(self.code_block(&lang, &code, cx)),
            };
        }
        out
    }

    /// A fenced code block as GitLab draws it: the code background, a
    /// border, 12px padding, highlighted 14px mono on 22.4px lines.
    fn code_block(&self, lang: &str, code: &str, cx: &App) -> Div {
        let mut hl = Highlighter::new(&lang_path(lang));
        let lines = code.lines().map(|l| {
            let highlights: Vec<_> = hl
                .line(l)
                .into_iter()
                .map(|(r, t)| (r, HighlightStyle { color: Some(token(t)), ..Default::default() }))
                .collect();
            // An empty line still takes its height.
            let text = if l.is_empty() { " ".to_string() } else { l.to_string() };
            div().child(StyledText::new(SharedString::from(text)).with_highlights(highlights))
        });
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
        block.flex().flex_col().children(lines)
    }

    /// GitLab's "Suggested change": the lines it replaces and with what.
    fn suggestion(&self, s: &Suggestion, path: Option<&str>, cx: &App) -> Div {
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
            .children(lines.iter().map(|l| code_line(l, &mono, None)))
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
        out.push(self.note(first, path, first_avatar, window, cx).into_any_element());
        if !toggle {
            for n in &notes[1..] {
                out.push(self.note(n, path, true, window, cx).into_any_element());
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
                    out.push(self.note(n, path, true, window, cx).into_any_element());
                }
            }
        }
        out
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
