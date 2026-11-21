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
    /// A heading of this level (GitLab's "Text style"); 0: normal text.
    Heading(u8),
    /// "Add a quick action": a `/` starting a line.
    Quick,
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
            "quick" => Md::Quick,
            "h1" => Md::Heading(1),
            "h2" => Md::Heading(2),
            "h3" => Md::Heading(3),
            "h4" => Md::Heading(4),
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
        Md::Heading(0) => sel.trim_start_matches('#').trim_start().to_string(),
        Md::Heading(n) => lines(&|_| format!("{} ", "#".repeat(n as usize))),
        Md::Quick => {
            if at_line_start {
                "/".into()
            } else {
                "\n/".into()
            }
        }
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

    /// "Attach a file or image": pick files, upload each to the project,
    /// and put GitLab's markdown for them in box `b` at the cursor.
    pub(crate) fn attach(&mut self, b: MdBox, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let _ = this.update_in(cx, |v, window, cx| v.upload_files(b, paths, window, cx));
        })
        .detach();
    }

    /// Upload `paths` and insert their markdown in box `b`.
    pub(crate) fn upload_files(
        &mut self,
        b: MdBox,
        paths: Vec<std::path::PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let project = if b == MdBox::NewMr {
            self.new_mr.project
        } else {
            self.loc.project
        };
        let (Some(p), Some(c)) = (project, self.client.clone()) else {
            return;
        };
        cx.spawn_in(window, async move |this, cx| {
            let got = cx
                .background_spawn(async move {
                    paths
                        .iter()
                        .map(|f| c.upload(p, f))
                        .collect::<anyhow::Result<Vec<String>>>()
                })
                .await;
            let _ = this.update_in(cx, |v, window, cx| {
                match got {
                    Ok(mds) => {
                        let e = v.md_entity(b);
                        e.update(cx, |s, cx| {
                            s.insert(format!("{}\n", mds.join("\n")), window, cx);
                            s.focus(window, cx);
                        });
                    }
                    Err(e) => v.write_error = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// GitLab's "Text style": normal text or a heading, for the line.
    fn style_menu(&self, b: MdBox, cx: &mut Context<Self>) -> Div {
        let open = self.more.style_open == Some(b);
        let item = |label: &'static str, md: Md| {
            div()
                .id(SharedString::from(format!("md-style-{b:?}-{label}")))
                .px_3()
                .py_1()
                .cursor_pointer()
                .hover(|s| s.bg(c(RAISED)))
                .text_color(c(TEXT))
                .child(label)
                .on_click(cx.listener(move |v, _, window, cx| {
                    v.more.style_open = None;
                    v.md_apply(b, md, window, cx);
                }))
        };
        div()
            .relative()
            .child(
                div()
                    .id(SharedString::from(format!("md-style-{b:?}")))
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .h(px(28.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .text_size(px(13.))
                    .text_color(c(MUTED))
                    .hover(|s| s.bg(ca(0x89888d66)))
                    .child("Text style")
                    .child(app_icon("chevron-down", 12., c(MUTED)))
                    .on_click(cx.listener(move |v, _, _, cx| {
                        v.more.style_open = if v.more.style_open == Some(b) {
                            None
                        } else {
                            Some(b)
                        };
                        cx.notify();
                    })),
            )
            .when(open, |d| {
                d.child(
                    deferred(
                        div()
                            .id(SharedString::from(format!("md-style-menu-{b:?}")))
                            .absolute()
                            .top(px(30.))
                            .left_0()
                            .w(px(160.))
                            .py_1()
                            .rounded(px(8.))
                            .border_1()
                            .border_color(c(BORDER))
                            .bg(c(CARD))
                            .shadow_lg()
                            .occlude()
                            .on_mouse_down_out(cx.listener(|v, _, _, cx| {
                                v.more.style_open = None;
                                cx.notify();
                            }))
                            .child(item("Normal text", Md::Heading(0)))
                            .child(item("Heading 1", Md::Heading(1)))
                            .child(item("Heading 2", Md::Heading(2)))
                            .child(item("Heading 3", Md::Heading(3)))
                            .child(item("Heading 4", Md::Heading(4))),
                    )
                    .with_priority(2),
                )
            })
    }

    /// "Insert comment template": your saved replies, one inserted on a click.
    fn templates_menu(&self, b: MdBox, cx: &mut Context<Self>) -> Div {
        let open = self.more.templates_open == Some(b);
        let list = self.more.templates.clone();
        div()
            .relative()
            .child(
                crate::edit::icon_button(
                    SharedString::from(format!("md-templates-{b:?}")),
                    icon("comment-lines", 15., c(MUTED)),
                    "Insert comment template",
                )
                .on_click(cx.listener(move |v, _, window, cx| v.toggle_templates(b, window, cx))),
            )
            .when(open, |d| {
                let rows: Vec<AnyElement> = match &list {
                    None => vec![
                        div()
                            .px_3()
                            .py_1()
                            .text_color(c(MUTED))
                            .child("Loading…")
                            .into_any_element(),
                    ],
                    Some(l) if l.is_empty() => vec![
                        div()
                            .px_3()
                            .py_1()
                            .text_color(c(MUTED))
                            .child("No comment templates. Make them in your GitLab preferences.")
                            .into_any_element(),
                    ],
                    Some(l) => l
                        .iter()
                        .enumerate()
                        .map(|(i, (name, text))| {
                            let text = text.clone();
                            div()
                                .id(SharedString::from(format!("md-template-{b:?}-{i}")))
                                .flex()
                                .flex_col()
                                .px_3()
                                .py_1()
                                .cursor_pointer()
                                .hover(|s| s.bg(c(RAISED)))
                                .child(
                                    div()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(c(TEXT))
                                        .child(name.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(c(MUTED))
                                        .truncate()
                                        .child(text.lines().next().unwrap_or("").to_string()),
                                )
                                .on_click(cx.listener(move |v, _, window, cx| {
                                    v.more.templates_open = None;
                                    let e = v.md_entity(b);
                                    let t = text.clone();
                                    e.update(cx, |s, cx| {
                                        s.replace(t, window, cx);
                                        s.focus(window, cx);
                                    });
                                    cx.notify();
                                }))
                                .into_any_element()
                        })
                        .collect(),
                };
                d.child(
                    deferred(
                        div()
                            .id(SharedString::from(format!("md-templates-menu-{b:?}")))
                            .absolute()
                            .top(px(30.))
                            .right_0()
                            .w(px(300.))
                            .py_1()
                            .rounded(px(8.))
                            .border_1()
                            .border_color(c(BORDER))
                            .bg(c(CARD))
                            .shadow_lg()
                            .occlude()
                            .on_mouse_down_out(cx.listener(|v, _, _, cx| {
                                v.more.templates_open = None;
                                cx.notify();
                            }))
                            .children(rows),
                    )
                    .with_priority(2),
                )
            })
    }

    /// Open (fetching once) or close box `b`'s template menu.
    pub(crate) fn toggle_templates(
        &mut self,
        b: MdBox,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.more.templates_open == Some(b) {
            self.more.templates_open = None;
            cx.notify();
            return;
        }
        self.more.templates_open = Some(b);
        if self.more.templates.is_none()
            && let Some(c) = self.client.clone()
        {
            cx.spawn_in(window, async move |this, cx| {
                let got = cx.background_spawn(async move { c.saved_replies() }).await;
                let _ = this.update(cx, |v, cx| {
                    v.more.templates = Some(got.unwrap_or_default());
                    cx.notify();
                });
            })
            .detach();
        }
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

    /// Box `b`'s text as GitLab renders it (Preview, and rich text).
    fn md_rendered(&self, b: MdBox, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
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
        let style = self.style_menu(b, cx);
        let templates = (!self.is_local()).then(|| self.templates_menu(b, cx));
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
            .child(style)
            .child(tool(Md::Bold, "bold", "Add bold text (⌘B)"))
            .child(tool(Md::Italic, "italic", "Add italic text (⌘I)"))
            .child(tool(Md::Strike, "strikethrough", "Add strikethrough text"))
            .child(tool(Md::Quote, "quote", "Insert a quote"))
            .child(tool(Md::Code, "code", "Insert code"))
            .child(tool(Md::Link, "md-link", "Add a link (⌘K)"))
            .child(tool(Md::Bullets, "md-list", "Add a bullet list"))
            .child(tool(Md::Numbers, "list-ordered", "Add a numbered list"))
            .child(tool(Md::Tasks, "md-tasks", "Add a checklist"))
            .child(tool(Md::Table, "table", "Add a table"))
            .when(!self.is_local(), |d| {
                d.child(tool(Md::Quick, "at-sign", "Add a quick action"))
            })
            .children(templates)
            .when(!self.is_local(), |d| {
                d.child(
                    crate::edit::icon_button(
                        SharedString::from(format!("md-{b:?}-attach")),
                        icon("paperclip", 15., c(MUTED)),
                        "Attach a file or image",
                    )
                    .on_click(cx.listener(move |v, _, window, cx| v.attach(b, window, cx))),
                )
            });
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
        let rich = self.more.rich.contains(&b);
        let body: AnyElement = if previewing {
            self.md_rendered(b, window, cx)
        } else if rich {
            // "Rich text": what's typed, formatted beside it as it's typed.
            let shown = self.md_rendered(b, window, cx);
            div()
                .flex()
                .gap_3()
                .child(div().flex_1().min_w(px(0.)).child(input))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .pl_3()
                        .border_l_1()
                        .border_color(c(RAISED))
                        .child(shown),
                )
                .into_any_element()
        } else {
            input
        };
        let switch = div()
            .id(SharedString::from(format!("md-rich-{b:?}")))
            .pt_1()
            .text_size(px(12.))
            .text_color(c(MUTED))
            .cursor_pointer()
            .hover(|s| s.text_color(c(TEXT)))
            .child(if rich {
                "Switch to plain text editing"
            } else {
                "Switch to rich text editing"
            })
            .on_click(cx.listener(move |v, _, _, cx| {
                if !v.more.rich.remove(&b) {
                    v.more.rich.insert(b);
                }
                cx.notify();
            }));
        div()
            .flex()
            .flex_col()
            .child(head)
            .child(body)
            .when(!previewing, |d| d.child(switch))
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
