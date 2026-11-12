//! Selecting code with the mouse and copying it with ⌘C, as in GitLab's
//! page: in the diff (one side at a time, side by side), in the overview's
//! snippets and in code blocks. Markdown text selects on its own
//! (gpui-component's TextView); this is for the code we draw ourselves.
//!
//! A selection runs from an anchor to the head, each a row and a byte in
//! it, within one group of rows. The diff copies from its data, so a
//! selection longer than the screen copies whole; a block copies the lines
//! it drew.

use crate::{discussion::overlay, style::*, view::GitLabView};
use gpui::{prelude::*, *};
#[allow(unused_imports)]
use crate::style::px;
use std::ops::Range;

/// The rows a selection stays in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    /// The Changes tab's rows: side 0 (inline, or the old side), or 1.
    Diff(u8),
    /// One code block, snippet or suggestion, by a hash of what it is.
    Block(u64),
}

impl Group {
    pub fn block(key: impl std::hash::Hash) -> Group {
        use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher};
        Group::Block(BuildHasherDefault::<DefaultHasher>::default().hash_one(key))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pos {
    pub row: usize,
    pub col: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Selection {
    pub group: Group,
    pub anchor: Pos,
    pub head: Pos,
    /// The button's still down: moves extend it.
    pub dragging: bool,
}

impl Selection {
    /// First and last position, whichever way it was dragged.
    pub fn span(&self) -> (Pos, Pos) {
        (self.anchor.min(self.head), self.anchor.max(self.head))
    }

    /// The selected bytes of `row` in `group`, whose text is `len` long:
    /// all of it between the ends, part of it at them.
    pub fn range(&self, group: Group, row: usize, len: usize) -> Option<Range<usize>> {
        let (a, b) = self.span();
        if group != self.group || row < a.row || row > b.row || a == b {
            return None;
        }
        let start = if row == a.row { a.col.min(len) } else { 0 };
        let end = if row == b.row { b.col.min(len) } else { len };
        Some(start..end.max(start))
    }

    /// The selected text, given each row's text in the group (None: a row
    /// that isn't code, such as a thread between two lines, skipped).
    pub fn text(&self, row_text: impl Fn(usize) -> Option<String>) -> String {
        let (a, b) = self.span();
        let mut out: Vec<String> = vec![];
        for row in a.row..=b.row {
            let Some(t) = row_text(row) else { continue };
            let r = self.range(self.group, row, t.len()).unwrap_or(0..0);
            let (start, end) = (floor(&t, r.start), floor(&t, r.end));
            out.push(t[start..end].to_string());
        }
        out.join("\n")
    }
}

/// `i`, or the nearest char boundary below it.
fn floor(s: &str, i: usize) -> usize {
    (0..=i.min(s.len())).rev().find(|i| s.is_char_boundary(*i)).unwrap_or(0)
}

/// A selectable line drawn this frame: where, and its laid-out text.
pub struct Drawn {
    pub group: Group,
    pub row: usize,
    pub at: std::rc::Rc<std::cell::Cell<Option<Bounds<Pixels>>>>,
    pub layout: TextLayout,
}

/// The highlight of selected text: the browser's selection over GitLab's dark theme.
const SELECTION: u32 = 0x1f75cb99;

impl GitLabView {
    /// A line of code that can be selected: row `row` of `group`, its text
    /// with `highlights` and, if selected, the selection over them.
    pub(crate) fn sel_text(
        &self,
        text: SharedString,
        highlights: Vec<(Range<usize>, HighlightStyle)>,
        group: Group,
        row: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Group::Block(_) = group {
            self.sel_lines.borrow_mut().insert((group, row), text.to_string());
        }
        let mut highlights = highlights;
        if let Some(r) = self.sel.and_then(|s| s.range(group, row, text.len())).filter(|r| !r.is_empty()) {
            let (start, end) = (floor(&text, r.start), floor(&text, r.end));
            highlights = overlay(highlights, start..end, ca(SELECTION));
        }
        let styled = StyledText::new(text).with_highlights(highlights);
        let down = styled.layout().clone();
        // Where this line lands this frame: a drag finds the row under the
        // pointer by it (`follow`), since rows don't see moves while the
        // button's down.
        let drawn = std::rc::Rc::new(std::cell::Cell::new(None));
        self.sel_drawn.borrow_mut().push(Drawn { group, row, at: drawn.clone(), layout: styled.layout().clone() });
        div()
            .relative()
            .w_full()
            .cursor(CursorStyle::IBeam)
            .child(styled)
            .child(canvas(move |bounds, _, _| drawn.set(Some(bounds)), |_, _, _, _| {}).absolute().top_0().left_0().size_full())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |v, ev: &MouseDownEvent, window, cx| {
                    let p = Pos { row, col: index(&down, ev.position) };
                    v.sel = Some(match v.sel {
                        // Shift-click extends, as in a browser.
                        Some(s) if ev.modifiers.shift && s.group == group => Selection { head: p, dragging: true, ..s },
                        _ => Selection { group, anchor: p, head: p, dragging: true },
                    });
                    // The tab keeps the keys (for ⌘C); nothing above takes the click.
                    v.sel_started = true;
                    window.focus(&v.focus);
                    window.prevent_default();
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    /// A drag's move: the head to the row under the pointer, or the nearest
    /// one of the selection's group (above the first: its start; below the
    /// last: its end).
    pub(crate) fn follow(&mut self, p: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(s) = self.sel.filter(|s| s.dragging) else { return };
        let head = {
            let drawn = self.sel_drawn.borrow();
            let lines: Vec<(&Drawn, Bounds<Pixels>)> =
                drawn.iter().filter(|d| d.group == s.group).filter_map(|d| Some((d, d.at.get()?))).collect();
            let Some(first) = lines.iter().min_by_key(|(d, _)| d.row) else { return };
            let last = lines.iter().max_by_key(|(d, _)| d.row).unwrap();
            if let Some((d, _)) = lines.iter().find(|(_, b)| p.y >= b.top() && p.y < b.bottom()) {
                Pos { row: d.row, col: index(&d.layout, p) }
            } else if p.y < first.1.top() {
                Pos { row: first.0.row, col: 0 }
            } else if p.y >= last.1.bottom() {
                Pos { row: last.0.row, col: usize::MAX }
            } else {
                // Between two lines (a thread between them): the one above's end.
                let above = lines.iter().filter(|(_, b)| b.bottom() <= p.y).max_by_key(|(d, _)| d.row).unwrap_or(first);
                Pos { row: above.0.row, col: usize::MAX }
            }
        };
        if let Some(s) = self.sel.as_mut().filter(|s| s.head != head) {
            s.head = head;
            cx.notify();
        }
    }

    /// ⌘C with code selected: the selection to the clipboard. False when
    /// there's nothing to copy (the key is someone else's then).
    pub(crate) fn copy_selection(&self, cx: &mut Context<Self>) -> bool {
        let Some(s) = self.sel.filter(|s| s.anchor != s.head) else { return false };
        let text = match s.group {
            Group::Diff(side) => s.text(|row| self.diff_row_text(row, side)),
            group => {
                let lines = self.sel_lines.borrow();
                s.text(|row| lines.get(&(group, row)).cloned())
            }
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        true
    }

    /// The code on a row of the Changes tab, on `side` side by side.
    pub(crate) fn diff_row_text(&self, row: usize, side: u8) -> Option<String> {
        use crate::changes::Row;
        let r = self.shown()?;
        let lines = |f: usize, h: usize| r.files.get(f).and_then(|f| f.body().hunks().get(h)).map(|h| h.lines.clone());
        match *self.rows.get(row)? {
            Row::Line(f, h, l) => lines(f, h)?.get(l).map(|l| l.text.clone()),
            Row::Pair(f, h, l, rt) => {
                let i = if side == 0 { l } else { rt }?;
                lines(f, h)?.get(i).map(|l| l.text.clone())
            }
            Row::Context(f, new, _) => {
                let t = self.contents.get(&f)?.ready()?.clone();
                t.lines.get(new as usize - 1).cloned()
            }
            _ => None,
        }
    }

    /// The button's up: the selection stays, no longer following the pointer.
    pub(crate) fn end_selection(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.sel.as_mut().filter(|s| s.dragging) {
            s.dragging = false;
            if s.anchor == s.head {
                self.sel = None;
            }
            cx.notify();
        }
    }
}

/// The byte under `p` in a laid-out text (the nearest one, off its end).
fn index(layout: &TextLayout, p: Point<Pixels>) -> usize {
    layout.index_for_position(p).unwrap_or_else(|i| i)
}

#[cfg(test)]
mod tests {
    use super::{Group, Pos, Selection};

    #[test]
    fn a_selection_copies_what_it_covers() {
        let rows = ["fn main() {", "    let x = 1;", "", "}"];
        let at = |row, col| Pos { row, col };
        let sel = |a, b| Selection { group: Group::Diff(0), anchor: a, head: b, dragging: false };
        let text = |s: Selection| s.text(|r| rows.get(r).map(|t| t.to_string()));
        // Within a line, either way round.
        assert_eq!(text(sel(at(1, 4), at(1, 7))), "let");
        assert_eq!(text(sel(at(1, 7), at(1, 4))), "let");
        // Across lines: the tail of the first, whole middles (empty ones too), the head of the last.
        assert_eq!(text(sel(at(0, 3), at(3, 1))), "main() {\n    let x = 1;\n\n}");
        // Rows that aren't code are skipped.
        let s = sel(at(0, 0), at(2, 0));
        assert_eq!(s.text(|r| (r != 1).then(|| rows[r].to_string())), "fn main() {\n");
        // Another group's rows aren't selected.
        assert_eq!(sel(at(0, 0), at(1, 3)).range(Group::Diff(1), 0, 11), None);
        assert_eq!(sel(at(0, 0), at(1, 3)).range(Group::Diff(0), 0, 11), Some(0..11));
    }
}
