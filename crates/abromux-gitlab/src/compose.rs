//! GitLab's comment editor around each text box: "Write" and "Preview",
//! and the formatting toolbar (bold, italic, strikethrough, quote, code,
//! link, lists, a task list, a table) that wraps what's selected, or puts
//! the markup in at the cursor. A comment on a line of the diff also has
//! "Insert suggestion", which writes GitLab's ```` ```suggestion ```` block
//! with the line's text in it to edit.

#[allow(unused_imports)]
use crate::style::px;
use crate::{style::*, view::GitLabView};
use gpui::{prelude::*, *};
use gpui_component::input::InputState;

/// Which box an editor is around.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MdBox {
    Comment,
    Reply,
    Line,
    Note,
    Description,
    /// A new merge request's description.
    NewMr,
}

impl MdBox {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "comment" => MdBox::Comment,
            "reply" => MdBox::Reply,
            "line" => MdBox::Line,
            "note" => MdBox::Note,
            "description" => MdBox::Description,
            _ => return None,
        })
    }
}

/// A toolbar button's markup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Md {
    Bold,
    Italic,
    Strike,
    Quote,
    Code,
    Link,
    Bullets,
    Numbers,
    Tasks,
    Table,
    Suggestion,
}

impl Md {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "bold" => Md::Bold,
            "italic" => Md::Italic,
            "strike" => Md::Strike,
            "quote" => Md::Quote,
            "code" => Md::Code,
            "link" => Md::Link,
            "bullets" => Md::Bullets,
            "numbers" => Md::Numbers,
            "tasks" => Md::Tasks,
            "table" => Md::Table,
            "suggestion" => Md::Suggestion,
            _ => return None,
        })
    }
}

const TABLE: &str =
    "| header | header |\n| ------ | ------ |\n|        |        |\n|        |        |\n";

/// `sel` with `md` applied, as GitLab's toolbar does: wrapped for inline
/// markup, each line prefixed for quotes and lists. `at_line_start`: the
/// cursor is at the start of a line (else block markup starts a new one).
pub fn apply(md: Md, sel: &str, at_line_start: bool) -> String {
    let wrap = |l: &str, r: &str, placeholder: &str| {
        format!("{l}{}{r}", if sel.is_empty() { placeholder } else { sel })
    };
    let lines = |prefix: &dyn Fn(usize) -> String| {
        let body = if sel.is_empty() {
            prefix(0)
        } else {
            sel.lines()
                .enumerate()
                .map(|(i, l)| format!("{}{l}", prefix(i)))
                .collect::<Vec<_>>()
                .join("\n")
        };
        if at_line_start {
            body
        } else {
            format!("\n{body}")
        }
    };
    match md {
        Md::Bold => wrap("**", "**", "bold text"),
        Md::Italic => wrap("_", "_", "italic text"),
        Md::Strike => wrap("~~", "~~", "strikethrough text"),
        Md::Code if sel.contains('\n') => format!("```\n{sel}\n```"),
        Md::Code => wrap("`", "`", "code"),
        Md::Link => format!("[{}](url)", if sel.is_empty() { "text" } else { sel }),
        Md::Quote => lines(&|_| "> ".into()),
        Md::Bullets => lines(&|_| "- ".into()),
        Md::Numbers => lines(&|i| format!("{}. ", i + 1)),
        Md::Tasks => lines(&|_| "- [ ] ".into()),
        Md::Table => {
            if at_line_start {
                TABLE.into()
            } else {
                format!("\n\n{TABLE}")
            }
        }
        // Filled in by the caller, which knows the lines.
        Md::Suggestion => sel.to_string(),
    }
}

/// GitLab's suggestion block for `text`, the commented line(s).
pub fn suggestion_block(text: &str, lines: usize) -> String {
    let above = lines.saturating_sub(1);
    format!("```suggestion:-{above}+0\n{text}\n```")
}

impl GitLabView {
    pub(crate) fn md_entity(&self, b: MdBox) -> Entity<InputState> {
        match b {
            MdBox::Comment => self.boxes.comment.clone(),
            MdBox::Reply => self.boxes.reply.clone(),
            MdBox::Line => self.boxes.line.clone(),
            MdBox::Note => self.boxes.note.clone(),
            MdBox::Description => self.boxes.description.clone(),
            MdBox::NewMr => self.new_mr.description.clone(),
        }
    }

    /// Apply a toolbar button to box `b`: on what's selected there, or at
    /// the cursor.
    pub(crate) fn md_apply(
        &mut self,
        b: MdBox,
        md: Md,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = if md == Md::Suggestion {
            match self.range_text().or_else(|| self.commented_text(cx)) {
                Some((t, n)) => suggestion_block(&t, n),
                None => return,
            }
        } else {
            String::new()
        };
        self.more.previewing.remove(&b);
        let e = self.md_entity(b);
        e.update(cx, |s, cx| {
            let sel = s
                .selected_text_range(true, window, cx)
                .map(|r| r.range)
                .and_then(|r| s.text_for_range(r, &mut None, window, cx))
                .unwrap_or_default();
            let start = s.cursor_position().character == 0 || !sel.is_empty();
            let new = if md == Md::Suggestion {
                if start { text } else { format!("\n{text}") }
            } else {
                apply(md, &sel, start)
            };
            s.replace(new, window, cx);
            s.focus(window, cx);
        });
        cx.notify();
    }

    /// The text of the line a new thread is being written on, and how many
    /// lines that is.
    fn commented_text(&self, _cx: &App) -> Option<(String, usize)> {
        let (fi, old, new) = self.commenting?;
        let r = self.shown()?;
        let f = r.files.get(fi)?;
        let line = f
            .body()
            .hunks()
            .iter()
            .flat_map(|h| &h.lines)
            .find(|l| l.old == old && l.new == new)?;
        Some((line.text.clone(), 1))
    }

    /// Box `b` with GitLab's editor around it: Write and Preview, the
    /// toolbar, then the box (or, previewing, what it renders as).
    pub(crate) fn md_editor(
        &self,
        b: MdBox,
        input: AnyElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let previewing = self.more.previewing.contains(&b);
        let tab = |id: &'static str, label: &'static str, on: bool| {
            div()
                .id(SharedString::from(format!("{id}-{b:?}")))
                .px_2()
                .py(px(2.))
                .rounded(px(6.))
                .cursor_pointer()
                .text_size(px(14.))
                .text_color(c(if on { HEADING } else { MUTED }))
                .when(on, |d| {
                    d.bg(ca(0x89888d40)).font_weight(FontWeight::SEMIBOLD)
                })
                .hover(|s| s.bg(ca(0x89888d33)))
                .child(label)
        };
        let tool = |md: Md, name: &'static str, tip: &'static str| {
            crate::edit::icon_button(
                SharedString::from(format!("md-{b:?}-{md:?}")),
                icon(name, 15., c(MUTED)),
                tip,
            )
            .on_click(cx.listener(move |v, _, window, cx| v.md_apply(b, md, window, cx)))
        };
        let suggest = b == MdBox::Line;
        let toolbar = div()
            .flex()
            .items_center()
            .flex_wrap()
            .gap(px(2.))
            .when(suggest, |d| {
                d.child(tool(Md::Suggestion, "suggest", "Insert suggestion"))
            })
            .child(tool(Md::Bold, "bold", "Add bold text (⌘B)"))
            .child(tool(Md::Italic, "italic", "Add italic text (⌘I)"))
            .child(tool(Md::Strike, "strikethrough", "Add strikethrough text"))
            .child(tool(Md::Quote, "quote", "Insert a quote"))
            .child(tool(Md::Code, "code", "Insert code"))
            .child(tool(Md::Link, "md-link", "Add a link (⌘K)"))
            .child(tool(Md::Bullets, "md-list", "Add a bullet list"))
            .child(tool(Md::Numbers, "list-ordered", "Add a numbered list"))
            .child(tool(Md::Tasks, "md-tasks", "Add a checklist"))
            .child(tool(Md::Table, "table", "Add a table"));
        let head = div()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .pb_1()
            .mb_1()
            .border_b_1()
            .border_color(c(RAISED))
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(tab("md-write", "Write", !previewing).on_click(cx.listener(
                        move |v, _, window, cx| {
                            v.more.previewing.remove(&b);
                            let e = v.md_entity(b);
                            e.update(cx, |s, cx| s.focus(window, cx));
                            cx.notify();
                        },
                    )))
                    .child(
                        tab("md-preview", "Preview", previewing).on_click(cx.listener(
                            move |v, _, _, cx| {
                                v.more.previewing.insert(b);
                                cx.notify();
                            },
                        )),
                    ),
            )
            .when(!previewing, |d| d.child(toolbar));
        let body: AnyElement = if previewing {
            let text = self.md_entity(b).read(cx).value().to_string();
            if text.trim().is_empty() {
                div()
                    .py_2()
                    .text_color(c(MUTED))
                    .child("Nothing to preview.")
                    .into_any_element()
            } else {
                let path = self.commenting.and_then(|(fi, ..)| {
                    self.shown()
                        .and_then(|r| r.files.get(fi).map(|f| f.path().to_string()))
                });
                let mut suggestions =
                    preview_suggestions(&text, self.commenting.and_then(|(_, o, n)| n.or(o)));
                if let Some((old, _)) = self.commented_text(cx) {
                    for s in &mut suggestions {
                        s.from_content = format!("{old}\n");
                    }
                }
                div()
                    .py_1()
                    .min_h(px(60.))
                    .child(self.markdown(
                        ("md-preview", b as usize),
                        &text,
                        &suggestions,
                        path.as_deref(),
                        window,
                        cx,
                    ))
                    .into_any_element()
            }
        } else {
            input
        };
        div().flex().flex_col().child(head).child(body)
    }
}

/// The suggestions a preview shows, as GitLab will parse them: each
/// ```` ```suggestion ```` block, replacing the line commented on with its
/// text (the caller fills in the old text, which it knows).
fn preview_suggestions(text: &str, line: Option<u32>) -> Vec<crate::types::Suggestion> {
    let mut out = vec![];
    let mut it = text.lines();
    while let Some(l) = it.next() {
        if l.trim_start().starts_with("```suggestion") {
            let mut to = String::new();
            for b in it.by_ref() {
                if b.trim_start().starts_with("```") {
                    break;
                }
                to.push_str(b);
                to.push('\n');
            }
            let at = line.unwrap_or(1);
            out.push(crate::types::Suggestion {
                from_line: at,
                to_line: at,
                to_content: to,
                ..Default::default()
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{Md, apply, preview_suggestions, suggestion_block};

    #[test]
    fn wraps_and_prefixes_as_gitlab_does() {
        assert_eq!(apply(Md::Bold, "x", false), "**x**");
        assert_eq!(apply(Md::Bold, "", false), "**bold text**");
        assert_eq!(apply(Md::Code, "a\nb", false), "```\na\nb\n```");
        assert_eq!(apply(Md::Link, "site", false), "[site](url)");
        assert_eq!(apply(Md::Bullets, "a\nb", true), "- a\n- b");
        assert_eq!(apply(Md::Numbers, "a\nb", true), "1. a\n2. b");
        assert_eq!(apply(Md::Tasks, "", false), "\n- [ ] ");
        assert_eq!(apply(Md::Quote, "q", true), "> q");
    }

    #[test]
    fn suggestion_blocks() {
        assert_eq!(suggestion_block("  x", 1), "```suggestion:-0+0\n  x\n```");
        let s = preview_suggestions("Try:\n```suggestion:-0+0\ny\n```\n", Some(4));
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].to_content, "y\n");
        assert_eq!(s[0].from_line, 4);
    }
}
