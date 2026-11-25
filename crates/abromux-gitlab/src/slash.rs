//! GitLab's quick actions in a comment: typing `/` at the start of a line
//! lists the commands a merge request takes (`/label ~bug`, `/approve`,
//! `/spend 1h` …), narrowing as you type; ↑ ↓ pick, Enter or Tab puts the
//! command in, Esc closes the list. The toolbar's "Add a quick action"
//! starts one. GitLab runs them when the comment is posted.

#[allow(unused_imports)]
use crate::style::px;
use crate::{mention::Field, style::*, view::GitLabView};
use gpui::{prelude::*, *};
use gpui_component::input::InputState;
use std::ops::Range;

/// The quick actions of a merge request, as GitLab lists them: the
/// command, its arguments, what it does.
pub const COMMANDS: &[(&str, &str, &str)] = &[
    ("approve", "", "Approve a merge request"),
    ("assign", "@user", "Assign"),
    ("assign_reviewer", "@user", "Assign reviewers"),
    ("award", ":emoji:", "Toggle emoji reaction"),
    ("cc", "@user", "Mention a user"),
    ("close", "", "Close this merge request"),
    (
        "copy_metadata",
        "#item | !merge_request",
        "Copy labels and milestone from another item",
    ),
    ("done", "", "Mark to-do item as done"),
    ("draft", "", "Set the Draft status"),
    ("estimate", "<1w 3d 2h 14m>", "Set time estimate"),
    ("internal_note", "", "Make this comment an internal note"),
    ("label", "~label1 ~\"label 2\"", "Add labels"),
    ("lock", "", "Lock the discussion"),
    ("merge", "", "Merge (when the pipeline succeeds)"),
    ("milestone", "%\"milestone\"", "Set milestone"),
    ("react", ":emoji:", "Toggle emoji reaction"),
    ("ready", "", "Set the Ready status"),
    ("rebase", "", "Rebase source branch"),
    ("relabel", "~label1 ~\"label 2\"", "Replace all labels"),
    ("remove_estimate", "", "Remove time estimate"),
    ("remove_milestone", "", "Remove milestone"),
    ("remove_time_spent", "", "Remove spent time"),
    ("reopen", "", "Reopen this merge request"),
    ("request_changes", "", "Request changes"),
    ("request_review", "@user", "Request a review"),
    ("run_pipeline", "", "Run a pipeline"),
    ("shrug", "", "Add ¯\\＿(ツ)＿/¯"),
    (
        "spend",
        "time(1h30m | -1h30m) <date(YYYY-MM-DD)>",
        "Add or subtract spent time",
    ),
    ("submit_review", "", "Submit a review"),
    ("subscribe", "", "Subscribe"),
    ("tableflip", "", "Add (╯°□°)╯︵ ┻━┻"),
    ("target_branch", "<Local branch name>", "Set target branch"),
    ("title", "<New title>", "Change title"),
    ("todo", "", "Add a to-do item"),
    ("unapprove", "", "Unapprove a merge request"),
    ("unassign", "@user", "Remove assignee"),
    ("unassign_reviewer", "@user", "Remove reviewer"),
    ("unlabel", "~label1 ~\"label 2\"", "Remove labels"),
    ("unlock", "", "Unlock the discussion"),
    ("unsubscribe", "", "Unsubscribe"),
];

/// The list open under a box.
#[derive(Clone, Debug, PartialEq)]
pub struct Slash {
    pub field: Field,
    pub range: Range<usize>,
    pub query: String,
    pub selected: usize,
}

/// The `/command` being typed: a `/` starting the line, then letters and
/// `_` up to the cursor.
pub fn at_cursor(text: &str, cursor: usize) -> Option<(Range<usize>, String)> {
    let before = text.get(..cursor)?;
    let line = before.rfind('\n').map_or(0, |i| i + 1);
    let word = &before[line..];
    let q = word.strip_prefix('/')?;
    q.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
        .then(|| (line..cursor, q.to_string()))
}

/// The commands `query` could be, those starting with it first.
pub fn matches(query: &str) -> Vec<&'static (&'static str, &'static str, &'static str)> {
    let q = query.to_lowercase();
    let mut out: Vec<_> = COMMANDS
        .iter()
        .filter(|(c, ..)| c.starts_with(&q))
        .collect();
    out.extend(COMMANDS.iter().filter(|(c, _, d)| {
        !c.starts_with(&q) && (c.contains(&q) || d.to_lowercase().contains(&q))
    }));
    out.truncate(10);
    out
}

impl GitLabView {
    fn slash_box(&self, field: Field) -> Entity<InputState> {
        match field {
            Field::Comment => self.boxes.comment.clone(),
            Field::Reply => self.boxes.reply.clone(),
            Field::Line => self.boxes.line.clone(),
        }
    }

    /// The box changed: open, follow or close its list.
    pub(crate) fn update_slash(&mut self, field: Field, cx: &mut Context<Self>) {
        let b = self.slash_box(field);
        let b = b.read(cx);
        let at = if self.is_local() {
            None
        } else {
            at_cursor(&b.value(), b.cursor())
        };
        match at {
            Some((range, query)) if !matches(&query).is_empty() => {
                let selected = match &self.more.slash {
                    Some(s) if s.field == field && s.query == query => s.selected,
                    _ => 0,
                };
                self.more.slash = Some(Slash {
                    field,
                    range,
                    query,
                    selected,
                });
            }
            _ => {
                if self.more.slash.as_ref().is_some_and(|s| s.field == field) {
                    self.more.slash = None;
                }
            }
        }
        cx.notify();
    }

    pub(crate) fn move_slash(&mut self, by: isize, cx: &mut Context<Self>) {
        if let Some(s) = &mut self.more.slash {
            let n = matches(&s.query).len().max(1);
            s.selected = (s.selected.min(n - 1) as isize + by).rem_euclid(n as isize) as usize;
            cx.notify();
        }
    }

    /// Put the picked command in place of `/query`.
    pub(crate) fn accept_slash(
        &mut self,
        index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(s) = self.more.slash.take() else {
            return false;
        };
        let list = matches(&s.query);
        let Some((cmd, ..)) = list.get(
            index
                .unwrap_or(s.selected)
                .min(list.len().saturating_sub(1)),
        ) else {
            return false;
        };
        let insert = format!("/{cmd} ");
        self.slash_box(s.field).update(cx, |b, cx| {
            let text = b.value();
            if text
                .get(s.range.clone())
                .is_none_or(|t| t.strip_prefix('/') != Some(s.query.as_str()))
            {
                return;
            }
            let utf16 = |i: usize| text[..i].encode_utf16().count();
            let range = utf16(s.range.start)..utf16(s.range.end);
            EntityInputHandler::replace_text_in_range(b, Some(range), &insert, window, cx);
        });
        cx.notify();
        true
    }

    /// The list under box `field`, when it's open there.
    pub(crate) fn slash_list(&self, field: Field, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let s = self.more.slash.as_ref().filter(|s| s.field == field)?;
        let list = matches(&s.query);
        let selected = s.selected.min(list.len().saturating_sub(1));
        Some(
            div()
                .id("gl-quick-actions")
                .flex()
                .flex_col()
                .py_1()
                .rounded(px(8.))
                .border_1()
                .border_color(c(BORDER))
                .bg(c(RAISED))
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                })
                .children(list.into_iter().enumerate().map(|(i, (cmd, args, about))| {
                    div()
                        .id(("gl-quick", i))
                        .flex()
                        .flex_col()
                        .px_2()
                        .py_1()
                        .cursor_pointer()
                        .when(i == selected, |d| d.bg(ca(0x428fdc55)))
                        .hover(|s| s.bg(ca(0x89888d55)))
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(
                                    div()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(c(TEXT))
                                        .child(format!("/{cmd}")),
                                )
                                .child(div().text_color(c(MUTED)).child(args.to_string())),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(c(MUTED))
                                .child(about.to_string()),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |v, _, window, cx| {
                                window.prevent_default();
                                cx.stop_propagation();
                                v.accept_slash(Some(i), window, cx);
                            }),
                        )
                })),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{at_cursor, matches};

    #[test]
    fn finds_the_command_being_typed() {
        let at = |t: &str| at_cursor(t, t.len()).map(|(r, q)| (r.start, q));
        assert_eq!(at("/lab"), Some((0, "lab".into())));
        assert_eq!(at("Hi\n/app"), Some((3, "app".into())));
        assert_eq!(at("a /lab"), None);
        assert_eq!(at("/label ~x"), None);
        assert_eq!(matches("lab")[0].0, "label");
        assert!(matches("unlab").iter().any(|(c, ..)| *c == "unlabel"));
    }
}
