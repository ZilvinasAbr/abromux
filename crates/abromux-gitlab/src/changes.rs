//! The Changes tab as GitLab draws it: a file tree on the left, and every
//! file's diff in a card on the right, with threads under their lines.
//!
//! The diff side is a gpui `list`: rows (file headers, hunk headers, lines,
//! threads) are laid out only when on screen, so a 3000-line diff scrolls
//! like a short one.

use crate::{
    diff::{Body, changed_ranges},
    discussion::{code_line_with, half_with, line_highlights},
    select::Group,
    review::{File, Review},
    style::*,
    view::{Failure, GitLabView, Load},
};
use gpui::{prelude::*, *};
#[allow(unused_imports)]
use crate::style::px;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering};

/// The file tree's width until its edge is dragged, as GitLab's.
pub const TREE_W: Pixels = gpui::px(320.);
/// How narrow the tree can get, and how much room it leaves the diff.
const TREE_MIN: f32 = 200.;
const DIFF_MIN: f32 = 400.;
/// The width the tree was last dragged to, in any tab, for the next tab
/// opened (0: never dragged).
static LAST_TREE_W: AtomicU32 = AtomicU32::new(0);

pub fn last_tree_width() -> Pixels {
    match LAST_TREE_W.load(Ordering::Relaxed) {
        0 => TREE_W,
        w => gpui::px(f32::from_bits(w)),
    }
}

/// `w` kept between the tree's least width and what leaves the diff room
/// in a tab `tab` wide. Tree widths are at GitLab's size, before the tab's
/// zoom; `tab` is as measured.
fn clamp_tree_width(w: Pixels, tab: Pixels) -> Pixels {
    let max = (f32::from(tab) / zoom() - DIFF_MIN).max(TREE_MIN);
    gpui::px(f32::from(w).clamp(TREE_MIN, max))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    File(usize),
    /// "File moved.", "Binary file", …
    Message(usize),
    /// Hunk `h`'s header, with the buttons that show the unchanged lines
    /// above it (`hidden` of them).
    Hunk(usize, usize, u32),
    Line(usize, usize, usize),
    /// Side by side: hunk lines (old side, new side).
    Pair(usize, usize, Option<usize>, Option<usize>),
    /// An unchanged line shown by expanding: (file, new line, old line).
    Context(usize, u32, u32),
    /// The end of the diff, before the file's end: show more.
    Expand(usize),
    /// The form for a new thread on the line just above (edit.rs).
    Draft(usize),
    /// `review.files[f].threads[t]`
    Thread(usize, usize),
    /// The card's bottom edge.
    End(usize),
    Gap,
}

/// One version of an image: drawable if it decodes, its byte size and
/// width × height.
pub struct Shot {
    pub image: Option<std::sync::Arc<Image>>,
    pub bytes: usize,
    pub size: Option<(u32, u32)>,
}

impl Shot {
    fn of(bytes: Vec<u8>) -> Shot {
        let n = bytes.len();
        let size = image::ImageReader::new(std::io::Cursor::new(&bytes))
            .with_guessed_format()
            .ok()
            .and_then(|r| r.into_dimensions().ok());
        let image = crate::avatars::image_from_bytes(bytes).map(std::sync::Arc::new);
        Shot { image, bytes: n, size }
    }
}

/// A binary image file's two versions (none for the side it didn't exist on).
pub struct Picture {
    pub old: Option<Shot>,
    pub new: Option<Shot>,
}

impl Picture {
    /// "−41 B (210 B)": the change in size, and the new size.
    pub fn size_change(&self) -> Option<(i64, usize)> {
        let new = self.new.as_ref().map_or(0, |s| s.bytes);
        let old = self.old.as_ref().map_or(0, |s| s.bytes);
        Some((new as i64 - old as i64, new))
    }
}

fn is_image(path: &str) -> bool {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    matches!(ext.as_deref(), Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico"))
}

/// GitLab's file sizes: `512 B`, `1.00 KiB`, `2.50 MiB`.
pub fn human_size(n: usize) -> String {
    match n {
        n if n < 1024 => format!("{n} B"),
        n if n < 1024 * 1024 => format!("{:.2} KiB", n as f64 / 1024.),
        n => format!("{:.2} MiB", n as f64 / (1024. * 1024.)),
    }
}

/// A file's whole text at the merge request's head, for expanding.
pub struct FileText {
    pub lines: Vec<String>,
    pub spans: Vec<crate::highlight::Spans>,
}

impl Row {
    /// The file a row belongs to (a gap belongs to none).
    pub fn file(&self) -> Option<usize> {
        match *self {
            Row::File(f)
            | Row::Message(f)
            | Row::Hunk(f, _, _)
            | Row::Line(f, _, _)
            | Row::Pair(f, _, _, _)
            | Row::Context(f, _, _)
            | Row::Expand(f)
            | Row::Thread(f, _)
            | Row::End(f)
            | Row::Draft(f) => Some(f),
            Row::Gap => None,
        }
    }
}

/// What the user has folded, opened and expanded.
pub struct Shape<'a> {
    pub folded: &'a dyn Fn(&File) -> bool,
    /// Resolved threads shown, by id.
    pub open: &'a dyn Fn(&str) -> bool,
    /// Unchanged lines shown above hunk (file, hunk).
    pub above: &'a dyn Fn(usize, usize) -> u32,
    /// Unchanged lines shown after the last hunk.
    pub below: &'a dyn Fn(usize) -> u32,
    /// The file's line count, once its text is fetched.
    pub lines: &'a dyn Fn(usize) -> Option<u32>,
    /// The line a new thread is being written on: (file, old, new).
    pub draft: Option<crate::edit::LineKey>,
    /// Side by side instead of inline.
    pub parallel: bool,
}

/// The new file's last line in hunk `h`, and the old one's.
fn hunk_end(h: &crate::diff::Hunk) -> (u32, u32) {
    let new = h.lines.iter().filter_map(|l| l.new).max().unwrap_or(h.new_start.saturating_sub(1));
    let old = h.lines.iter().filter_map(|l| l.old).max().unwrap_or(h.old_start.saturating_sub(1));
    (new, old)
}

/// The rows for `review`.
pub fn rows(r: &Review, s: &Shape) -> Vec<Row> {
    let mut out = vec![];
    for (fi, f) in r.files.iter().enumerate() {
        out.push(Row::File(fi));
        if !(s.folded)(f) {
            match f.body() {
                Body::Text(hunks) => {
                    // Unchanged lines can only come from the new file's text.
                    let expandable = !f.meta.deleted_file && !f.meta.new_file;
                    let mut prev_new_end = 0;
                    for (hi, h) in hunks.iter().enumerate() {
                        // Unchanged lines between the previous hunk and this one.
                        let gap = if f.meta.new_file || f.meta.deleted_file {
                            0
                        } else {
                            h.new_start.saturating_sub(1).saturating_sub(prev_new_end)
                        };
                        let shown = if expandable && (s.lines)(fi).is_some() { (s.above)(fi, hi).min(gap) } else { 0 };
                        // The header shows while lines above are hidden: none at the
                        // top of a file, or once they're all shown (as in GitLab).
                        if gap > shown {
                            out.push(Row::Hunk(fi, hi, gap - shown));
                        }
                        let offset = h.old_start as i64 - h.new_start as i64;
                        for n in (h.new_start - shown)..h.new_start {
                            out.push(Row::Context(fi, n, (n as i64 + offset) as u32));
                        }
                        prev_new_end = hunk_end(h).0;
                        let threads = |out: &mut Vec<Row>, li: usize| {
                            for (ti, (th, tl, d)) in f.threads.iter().enumerate() {
                                if (*th, *tl) == (hi, li) && (!d.resolved() || (s.open)(&d.id)) {
                                    out.push(Row::Thread(fi, ti));
                                }
                            }
                        };
                        // The new-thread form goes after its line's threads.
                        let draft_on = |li: usize| {
                            let l = &h.lines[li];
                            s.draft == Some((fi, l.old, l.new))
                        };
                        if s.parallel {
                            for (l, r) in crate::diff::pairs(&h.lines) {
                                out.push(Row::Pair(fi, hi, l, r));
                                // Threads on either side, once for a context line.
                                for li in l.into_iter().chain(r.filter(|r| Some(*r) != l)) {
                                    threads(&mut out, li);
                                }
                                if l.into_iter().chain(r).any(draft_on) {
                                    out.push(Row::Draft(fi));
                                }
                            }
                        } else {
                            for li in 0..h.lines.len() {
                                out.push(Row::Line(fi, hi, li));
                                threads(&mut out, li);
                                if draft_on(li) {
                                    out.push(Row::Draft(fi));
                                }
                            }
                        }
                    }
                    // After the last hunk, up to the end of the file.
                    if expandable && let Some(last) = hunks.last() {
                        let (new_end, old_end) = hunk_end(last);
                        match (s.lines)(fi) {
                            Some(total) => {
                                let rest = total.saturating_sub(new_end);
                                let shown = (s.below)(fi).min(rest);
                                for n in new_end + 1..=new_end + shown {
                                    out.push(Row::Context(fi, n, (n - new_end) + old_end));
                                }
                                if rest > shown {
                                    out.push(Row::Expand(fi));
                                }
                            }
                            None => out.push(Row::Expand(fi)),
                        }
                    }
                }
                _ => out.push(Row::Message(fi)),
            }
            // A folded file is its rounded header alone.
            out.push(Row::End(fi));
        }
        out.push(Row::Gap);
    }
    out
}

/// The file browser's search: a case-insensitive substring, or a glob with
/// `*` (`*.rs`, `src/*`) matched against the whole path.
pub fn matches(path: &str, query: &str) -> bool {
    let (path, query) = (path.to_lowercase(), query.trim().to_lowercase());
    if query.is_empty() {
        return true;
    }
    if !query.contains('*') {
        return path.contains(&query);
    }
    let parts: Vec<&str> = query.split('*').collect();
    let mut rest = path.as_str();
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        let last = i == parts.len() - 1;
        if i == 0 {
            // Anchored at the start, unless the pattern starts with `*`.
            match rest.strip_prefix(part) {
                Some(r) => rest = r,
                None => return false,
            }
        } else if last {
            return rest.ends_with(part);
        } else {
            match rest.find(part) {
                Some(at) => rest = &rest[at + part.len()..],
                None => return false,
            }
        }
    }
    // Ends with `*`, or every part matched.
    true
}

/// The folders and files of the tree, in GitLab's order: folders first,
/// each sorted by name.
#[derive(Clone)]
pub enum TreeItem {
    /// `path`: the folder's full path, which folding goes by.
    Folder { name: String, depth: usize, path: String, folded: bool },
    File { index: usize, name: String, depth: usize },
}

/// The tree of the files matching `query` (see [`matches`]), without the
/// insides of folders in `folded`.
#[cfg(test)]
pub fn tree(r: &Review, query: &str) -> Vec<TreeItem> {
    tree_folded(r, query, &|_| false)
}

pub fn tree_folded(r: &Review, query: &str, folded: &dyn Fn(&str) -> bool) -> Vec<TreeItem> {
    #[derive(Default)]
    struct Dir {
        dirs: BTreeMap<String, Dir>,
        files: Vec<(String, usize)>,
    }
    let mut root = Dir::default();
    for (i, f) in r.files.iter().enumerate().filter(|(_, f)| matches(f.path(), query)) {
        let mut d = &mut root;
        let parts: Vec<&str> = f.path().split('/').collect();
        for p in &parts[..parts.len() - 1] {
            d = d.dirs.entry(p.to_string()).or_default();
        }
        d.files.push((parts[parts.len() - 1].to_string(), i));
    }
    fn walk(d: &Dir, depth: usize, at: &str, folded: &dyn Fn(&str) -> bool, out: &mut Vec<TreeItem>) {
        for (name, sub) in &d.dirs {
            let path = if at.is_empty() { name.clone() } else { format!("{at}/{name}") };
            let is_folded = folded(&path);
            out.push(TreeItem::Folder { name: name.clone(), depth, path: path.clone(), folded: is_folded });
            if !is_folded {
                walk(sub, depth + 1, &path, folded, out);
            }
        }
        let mut files = d.files.clone();
        files.sort();
        for (name, index) in files {
            out.push(TreeItem::File { index, name, depth });
        }
    }
    let mut out = vec![];
    walk(&root, 0, "", folded, &mut out);
    out
}

/// The card's sides; each row draws its part of them.
fn card(d: Div) -> Div {
    d.w_full().border_l_1().border_r_1().border_color(c(BORDER))
}

impl GitLabView {
    pub(crate) fn changes_page(&self, main: &std::rc::Rc<Review>, cx: &mut Context<Self>) -> AnyElement {
        self.current_drawn.set(self.current_file());
        // The picked versions' diffs, or the latest while those load.
        let r = &self.shown().unwrap_or_else(|| main.clone());
        let (added, removed) = r.files.iter().fold((0, 0), |(a, d), f| {
            let (x, y) = f.counts();
            (a + x, d + y)
        });
        // Wraps onto a second line in a narrow pane instead of squeezing.
        let compare = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(c(RAISED))
            .child(
                div()
                    .id("gl-tree-toggle")
                    .tooltip(hint("Toggle file browser"))
                    .flex_none()
                    .size(px(32.))
                    .rounded(px(8.))
                    .bg(ca(0x89888d66))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child(app_icon("panel-left", 16., c(TEXT)))
                    .on_click(cx.listener(|v, _, _, cx| {
                        v.show_tree = Some(!v.tree_shown());
                        cx.notify();
                    })),
            )
            .map(|bar| match self.compare.commit {
                Some(i) => bar.child(self.commit_banner(main, i, cx)),
                None => bar
                    .child(div().text_color(c(TEXT)).child("Compare"))
                    .child(self.version_picker(main, false, cx))
                    .child(div().text_color(c(TEXT)).child("and"))
                    .child(self.version_picker(main, true, cx)),
            })
            .child(div().flex_1())
            .child(self.fold_all_buttons(r, cx))
            .child(self.view_switch(cx))
            .child(icon("file", 16., c(MUTED)))
            .child(div().text_color(c(TEXT)).child(format!("{} file{}", r.files.len(), if r.files.len() == 1 { "" } else { "s" })))
            .child(div().font_weight(FontWeight::SEMIBOLD).text_color(c(GREEN)).child(format!("+{added}")))
            .child(div().font_weight(FontWeight::SEMIBOLD).text_color(c(RED)).child(format!("−{removed}")));
        let diff = div()
            .size_full()
            // Scrolling yourself ends a jump's hold on the view, and the
            // file being read is again the one at the top.
            .on_scroll_wheel(cx.listener(|v, _, _, cx| {
                v.jump = None;
                if v.current.take().is_some() {
                    cx.notify();
                }
            }))
            .child(list(self.diff_list.clone(), cx.processor(|v, ix: usize, window, cx| v.row(ix, window, cx))).size_full());
        // GitLab keeps the header of the file being read at the top.
        let top = self.diff_list.logical_scroll_top();
        let sticky = self.active_file().filter(|f| {
            self.rows.get(top.item_ix) != Some(&Row::File(*f)) || top.offset_in_item > px(0.)
        });
        // The next file's header pushes it up and out, as in GitLab, rather
        // than sliding under it.
        let push = (top.item_ix + 1..self.rows.len())
            .take(64)
            .find(|&i| matches!(self.rows[i], Row::File(_)))
            .and_then(|i| self.diff_list.bounds_for_item(i))
            .map(|b| {
                let below = b.top() - self.diff_list.viewport_bounds().top();
                (below - b.size.height).min(px(0.))
            })
            .unwrap_or(px(0.));
        let sticky = sticky.map(|f| {
            div()
                .absolute()
                .top(push)
                .left_0()
                .right_0()
                .bg(c(BG))
                .child(self.file_header(&r.files[f], f, cx).rounded_b(px(0.)))
        });
        let diff = match self.compare_loading() {
            Some(Some(e)) => div().p_4().text_color(c(RED)).child(e.to_string()),
            Some(None) => spinner("gl-compare-loading"),
            None => div().relative().size_full().overflow_hidden().child(diff).children(sticky),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(compare)
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .when(self.tree_shown(), |d| d.child(self.file_tree(r, cx)).child(self.tree_edge(cx)))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .h_full()
                            .flex()
                            .flex_col()
                            .pt_3()
                            .pr_4()
                            .pl(px(if self.tree_shown() { 0. } else { 16. }))
                            .children(self.compare.commit.and_then(|i| self.commit_card(r, i, cx)))
                            .child(div().flex_1().min_h(px(0.)).child(diff))
                            .children(self.file_pager(r, cx)),
                    ),
            )
            .into_any_element()
    }

    /// Viewing one commit: which, the ones before and after, and the way
    /// back to all of the merge request's changes.
    fn commit_banner(&self, r: &Review, i: usize, cx: &mut Context<Self>) -> Div {
        let Some(commit) = r.commits.get(i) else { return div() };
        let mono = self.mono(cx);
        // GitLab lists commits newest first: "previous" is the next one down.
        let (older, newer) = ((i + 1 < r.commits.len()).then_some(i + 1), i.checked_sub(1));
        let step = |id: &'static str, label: &'static str, back: bool, to: Option<usize>, cx: &mut Context<Self>| {
            let color = c(if to.is_some() { TEXT } else { MUTED });
            let chevron = app_icon(if back { "chevron-left" } else { "chevron-right" }, 14., color);
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_1()
                .h(px(32.))
                .px_3()
                .bg(ca(if to.is_some() { 0x89888d66 } else { 0x89888d26 }))
                .text_color(color)
                .when(to.is_some(), |d| d.cursor_pointer().hover(|s| s.bg(ca(0x89888d99))))
                // The chevron leads "Previous" and trails "Next".
                .child(label)
                .child(chevron)
                .when(back, |d| d.flex_row_reverse())
                .when_some(to, |d, to| d.on_click(cx.listener(move |v, _, _, cx| v.open_commit(to, cx))))
        };
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .text_color(c(TEXT))
            .child("Viewing commit")
            .child(div().font_family(mono).text_color(c(LINK)).child(commit.short_id.clone()))
            .child(
                div()
                    .flex()
                    .gap(px(1.))
                    .rounded(px(8.))
                    .overflow_hidden()
                    .child(step("gl-commit-prev", "Previous", true, older, cx))
                    .child(step("gl-commit-next", "Next", false, newer, cx)),
            )
            .child(
                crate::edit::secondary("gl-all-changes", "Show latest version")
                    .on_click(cx.listener(|v, _, _, cx| v.set_compare(crate::Compare::default(), cx))),
            )
    }

    /// Above a commit's changes, as GitLab has it: who wrote it, when, its
    /// sha to copy, and the rest of its message.
    fn commit_card(&self, r: &Review, i: usize, cx: &mut Context<Self>) -> Option<Div> {
        let cm = r.commits.get(i)?;
        let mono = self.mono(cx);
        let author = crate::mr::commit_author(r, cm);
        let when = if cm.authored_date.is_empty() { &cm.created_at } else { &cm.authored_date };
        let sha = cm.id.clone();
        Some(
            div()
                .mb_3()
                .p_4()
                .flex()
                .flex_col()
                .gap_3()
                .rounded(px(8.))
                .bg(c(RAISED))
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap_2()
                        .child(self.avatars.of(&author, 32.))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .flex()
                                .flex_col()
                                .child(div().font_weight(FontWeight::SEMIBOLD).text_color(c(TEXT)).child(cm.title.clone()))
                                .child(
                                    div()
                                        .flex()
                                        .gap_1()
                                        .text_size(px(12.))
                                        .line_height(px(18.))
                                        .child(div().text_color(c(TEXT)).child(author.name.clone()))
                                        .child(div().text_color(c(MUTED)).child("authored"))
                                        .child(crate::style::when(when).text_color(c(MUTED))),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(1.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .h(px(32.))
                                        .px_3()
                                        .rounded_l(px(8.))
                                        .bg(ca(0x89888d66))
                                        .font_family(mono.clone())
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(c(TEXT))
                                        .child(cm.short_id.clone()),
                                )
                                .child(
                                    div()
                                        .id("gl-commit-card-copy")
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .size(px(32.))
                                        .rounded_r(px(8.))
                                        .bg(ca(0x89888d66))
                                        .cursor_pointer()
                                        .hover(|s| s.bg(ca(0x89888d99)))
                                        .child(icon("copy", 14., c(TEXT)))
                                        .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(sha.clone()))),
                                ),
                        ),
                )
                .when_some(cm.body(), |d, body| {
                    d.child(
                        div()
                            .ml(px(40.))
                            .pl_3()
                            .border_l_2()
                            .border_color(c(BORDER))
                            .font_family(mono)
                            .text_size(px(13.))
                            .line_height(px(20.))
                            .text_color(c(TEXT))
                            .children(body.lines().map(|l| div().child(if l.is_empty() { " ".to_string() } else { l.to_string() }))),
                    )
                }),
        )
    }

    pub(crate) fn open_commit(&mut self, i: usize, cx: &mut Context<Self>) {
        self.loc.section = crate::Section::Changes;
        self.set_compare(crate::Compare { commit: Some(i), ..Default::default() }, cx);
    }

    /// GitLab's version picker: "Compare [main ▾] and [latest version ▾]".
    /// `head` is the right one (which version), else the left (against what).
    fn version_picker(&self, r: &Review, head: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let versions = &r.versions;
        // GitLab numbers versions from the oldest; the newest is "latest version".
        let number = |id: u64| versions.iter().position(|v| v.id == id).map(|i| versions.len() - i);
        let name = |id: u64| match number(id) {
            Some(n) if n == versions.len() => "latest version".to_string(),
            Some(n) => format!("version {n}"),
            None => "version ?".into(),
        };
        let label = match (head, self.compare.head, self.compare.base) {
            (true, None, _) => "latest version".to_string(),
            (true, Some(h), _) => name(h),
            (false, _, None) => r.mr.target_branch.clone(),
            (false, _, Some(b)) => name(b),
        };
        let open = self.menu == Some(head);
        let current_head = self.compare.head.or(versions.first().map(|v| v.id));
        let head_number = current_head.and_then(number).unwrap_or(versions.len());
        // The choices: (label, detail, compare).
        let mut items: Vec<(String, String, crate::Compare)> = vec![];
        let now = crate::time::now();
        if head {
            for v in versions {
                let pick = crate::Compare {
                    head: (Some(v.id) != versions.first().map(|f| f.id)).then_some(v.id),
                    ..Default::default()
                };
                let short = v.head_commit_sha.get(..8).unwrap_or_default();
                items.push((name(v.id), format!("{short} · {}", crate::time::ago(&v.created_at, now)), pick));
            }
        } else {
            items.push((format!("{} (base)", r.mr.target_branch), String::new(), crate::Compare { base: None, commit: None, ..self.compare }));
            // Only versions older than the head can be its base.
            for v in versions.iter().filter(|v| number(v.id).is_some_and(|n| n < head_number)) {
                let short = v.head_commit_sha.get(..8).unwrap_or_default();
                let pick = crate::Compare { base: Some(v.id), head: self.compare.head.or(current_head), commit: None };
                items.push((name(v.id), format!("{short} · {}", crate::time::ago(&v.created_at, now)), pick));
            }
        }
        let selected = |pick: &crate::Compare| {
            if head { pick.head == self.compare.head } else { pick.base == self.compare.base }
        };
        let menu = open.then(|| {
            deferred(
                div()
                    .id(if head { "gl-head-menu" } else { "gl-base-menu" })
                    .absolute()
                    .top(px(30.))
                    .left_0()
                    .min_w(px(240.))
                    .py_1()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(c(BORDER))
                    .bg(c(CARD))
                    .shadow_lg()
                    .occlude()
                    .on_mouse_down_out(cx.listener(|v, _, _, cx| {
                        v.menu = None;
                        cx.notify();
                    }))
                    .children(items.into_iter().enumerate().map(|(i, (title, detail, pick))| {
                        let on = selected(&pick);
                        div()
                            .id(i)
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_3()
                            .py_1()
                            .cursor_pointer()
                            .hover(|s| s.bg(c(RAISED)))
                            .child(div().w(px(14.)).when(on, |d| d.child(app_icon("check", 14., c(TEXT)))))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(div().text_color(c(TEXT)).child(title))
                                    .when(!detail.is_empty(), |d| {
                                        d.child(div().text_size(px(12.)).line_height(px(16.)).text_color(c(MUTED)).child(detail))
                                    }),
                            )
                            .on_click(cx.listener(move |v, _, _, cx| v.set_compare(pick, cx)))
                    })),
            )
            .with_priority(1)
        });
        div()
            .id(if head { "gl-head-version" } else { "gl-base-version" })
            .relative()
            .flex()
            .items_center()
            .gap_1()
            .px_1()
            .rounded(px(4.))
            .cursor_pointer()
            .hover(|s| s.bg(ca(0x89888d40)))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(c(TEXT))
            .child(label)
            .child(app_icon("chevron-down", 14., c(TEXT)))
            .on_click(cx.listener(move |v, _, _, cx| {
                v.menu = if v.menu == Some(head) { None } else { Some(head) };
                cx.notify();
            }))
            .children(menu)
    }

    /// One file at a time: on or off, keeping the file you're reading.
    pub(crate) fn set_single(&mut self, on: bool, cx: &mut Context<Self>) {
        self.single_file = on.then(|| self.active_file().unwrap_or(0));
        self.thread_cursor = None;
        self.rebuild_rows(cx);
        if let Some(f) = self.single_file {
            self.scroll_to_file(f, cx);
        }
    }

    /// One file at a time: GitLab's "Previous file" / "Next file" under it.
    fn file_pager(&self, r: &Review, cx: &mut Context<Self>) -> Option<Div> {
        let at = self.single_file?.min(r.files.len().saturating_sub(1));
        let n = r.files.len();
        let step = |id: &'static str, label: String, to: Option<usize>, cx: &mut Context<Self>| {
            div()
                .id(id)
                .flex()
                .items_center()
                .h(px(32.))
                .px_3()
                .rounded(px(8.))
                .bg(ca(if to.is_some() { 0x89888d66 } else { 0x89888d26 }))
                .text_color(c(if to.is_some() { TEXT } else { MUTED }))
                .when(to.is_some(), |d| d.cursor_pointer().hover(|s| s.bg(ca(0x89888d99))))
                .child(label)
                .when_some(to, |d, to| d.on_click(cx.listener(move |v, _, _, cx| v.scroll_to_file(to, cx))))
        };
        let name = |i: usize| r.files[i].path().rsplit('/').next().unwrap_or_default().to_string();
        let prev = at.checked_sub(1);
        let next = (at + 1 < n).then_some(at + 1);
        Some(
            div()
                .flex()
                .items_center()
                .gap_3()
                .py_3()
                .child(step("gl-file-prev", prev.map_or("Previous file".into(), |i| format!("← {}", name(i))), prev, cx))
                .child(div().flex_1().flex().justify_center().text_color(c(MUTED)).child(format!("File {} of {n}", at + 1)))
                .child(step("gl-file-next", next.map_or("Next file".into(), |i| format!("{} →", name(i))), next, cx)),
        )
    }

    /// GitLab's "Expand all files" and "Collapse all files" buttons, and its
    /// "Show one file at a time".
    fn fold_all_buttons(&self, r: &Review, cx: &mut Context<Self>) -> Div {
        let paths: Vec<String> = r.files.iter().map(|f| f.path().to_string()).collect();
        let button = |id: &'static str, name: &'static str, fold: bool, paths: Vec<String>, cx: &mut Context<Self>| {
            div()
                .id(id)
                .size(px(32.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .bg(ca(0x89888d66))
                .hover(|s| s.bg(ca(0x89888d99)))
                .child(icon(name, 16., c(TEXT)))
                .on_click(cx.listener(move |v, _, _, cx| {
                    for p in &paths {
                        v.folded.insert(p.clone(), fold);
                    }
                    v.rebuild_rows(cx);
                }))
        };
        div()
            .flex()
            .flex_none()
            .gap(px(1.))
            .mr_2()
            .rounded(px(8.))
            .overflow_hidden()
            .child(button("gl-expand-all", "expand-all", false, paths.clone(), cx).tooltip(hint("Expand all files")))
            .child(button("gl-collapse-all", "collapse-all", true, paths, cx).tooltip(hint("Collapse all files")))
            .child({
                let on = self.single_file.is_some();
                div()
                    .id("gl-one-file")
                    .size(px(32.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .bg(ca(if on { 0x89888d99 } else { 0x89888d66 }))
                    .hover(|s| s.bg(ca(0x89888d99)))
                    .child(icon("file", 16., c(TEXT)))
                    .tooltip(hint(if on { "Show all files" } else { "Show one file at a time" }))
                    .on_click(cx.listener(move |v, _, _, cx| v.set_single(!on, cx)))
            })
    }

    /// Inline or side by side, as GitLab's diff settings offer.
    fn view_switch(&self, cx: &mut Context<Self>) -> Div {
        let option = |label: &'static str, parallel: bool, cx: &mut Context<Self>| {
            let on = self.loc.parallel == parallel;
            div()
                .id(label)
                .flex()
                .items_center()
                .h(px(32.))
                .px_3()
                .cursor_pointer()
                .text_color(c(TEXT))
                .bg(if on { ca(0x89888d99) } else { ca(0x89888d40) })
                .when(on, |d| d.font_weight(FontWeight::SEMIBOLD))
                .hover(|s| s.bg(ca(0x89888d99)))
                .child(label)
                .on_click(cx.listener(move |v, _, _, cx| v.set_parallel(parallel, cx)))
        };
        div()
            .flex()
            .flex_none()
            .gap(px(1.))
            .mr_2()
            .rounded(px(8.))
            .overflow_hidden()
            .child(option("Inline", false, cx))
            .child(option("Side-by-side", true, cx))
    }

    pub(crate) fn set_parallel(&mut self, parallel: bool, cx: &mut Context<Self>) {
        if self.loc.parallel != parallel {
            self.loc.parallel = parallel;
            self.rebuild_rows(cx);
        }
    }

    /// GitLab's list / tree buttons over the file browser.
    fn tree_mode_switch(&self, cx: &mut Context<Self>) -> Div {
        let button = |id: &'static str, name: &'static str, flat: bool, cx: &mut Context<Self>| {
            let on = self.tree_flat == flat;
            div()
                .id(id)
                .size(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .bg(if on { ca(0x89888d99) } else { ca(0x89888d40) })
                .hover(|s| s.bg(ca(0x89888d99)))
                .child(icon(name, 14., c(TEXT)))
                .on_click(cx.listener(move |v, _, _, cx| {
                    v.tree_flat = flat;
                    cx.notify();
                }))
        };
        div()
            .flex()
            .gap(px(1.))
            .rounded(px(8.))
            .overflow_hidden()
            .child(button("gl-tree-list", "list", true, cx))
            .child(button("gl-tree-tree", "tree", false, cx))
    }

    pub(crate) fn set_tree_width(&mut self, w: Pixels) {
        self.tree_width = clamp_tree_width(w, self.width.get());
        LAST_TREE_W.store(f32::from(self.tree_width).to_bits(), Ordering::Relaxed);
    }

    /// While the tree's edge is dragged: its width follows the pointer.
    pub(crate) fn resize_tree(&mut self, p: Point<Pixels>, cx: &mut Context<Self>) {
        let Some((x0, w0)) = self.tree_resize else { return };
        self.set_tree_width(w0 + (p.x - x0) / zoom());
        cx.notify();
    }

    /// The grab area on the tree's right edge: drag to resize, double-click
    /// for the default width.
    fn tree_edge(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let dragging = self.tree_resize.is_some();
        div().relative().h_full().w(px(1.)).flex_none().when(dragging, |d| d.bg(c(LINK))).child(
            div()
                .id("gl-tree-edge")
                .absolute()
                .top_0()
                .left(px(-3.))
                .w(px(7.))
                .h_full()
                .cursor_col_resize()
                .hover(|s| s.bg(ca(0x428fdc66)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|v, ev: &MouseDownEvent, _, cx| {
                        if ev.click_count >= 2 {
                            v.tree_resize = None;
                            v.set_tree_width(TREE_W);
                        } else {
                            v.tree_resize = Some((ev.position.x, v.tree_width));
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }),
                ),
        )
    }

    fn file_tree(&self, r: &Review, cx: &mut Context<Self>) -> Div {
        let active = self.current_file();
        let query = self.tree_search.read(cx).value().to_string();
        // List mode: every file at the top level, by path.
        let items = if self.tree_flat {
            let mut files: Vec<(usize, &str)> =
                r.files.iter().enumerate().map(|(i, f)| (i, f.path())).filter(|(_, p)| matches(p, &query)).collect();
            files.sort_by_key(|(_, p)| *p);
            files
                .into_iter()
                .map(|(index, p)| TreeItem::File { index, name: p.rsplit('/').next().unwrap_or(p).to_string(), depth: 0 })
                .collect()
        } else {
            // A search shows every match, folded folders or not.
            let folded = |p: &str| query.trim().is_empty() && self.folded_dirs.contains(p);
            tree_folded(r, &query, &folded)
        };
        let none = items.is_empty();
        // Keep the file being read in view in the tree, as GitLab does: when
        // it changes, scroll the tree to its row.
        if active != self.tree_active.get() {
            self.tree_active.set(active);
            if let Some(ix) = items.iter().position(|it| matches!(it, TreeItem::File { index, .. } if Some(*index) == active)) {
                self.tree_scroll.scroll_to_item(ix, ScrollStrategy::Center);
            }
        }
        div()
            // A narrower tab than when it was dragged still leaves the diff room.
            .w(clamp_tree_width(self.tree_width, self.width.get()) * zoom())
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .px_4()
            .pt_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .pb_2()
                    .child(div().font_weight(FontWeight::SEMIBOLD).text_color(c(TEXT)).child("Files"))
                    .child(count_badge(r.files.len()))
                    .child(div().flex_1())
                    .child(self.tree_mode_switch(cx)),
            )
            .child(
                gpui_component::input::Input::new(&self.tree_search)
                    .prefix(app_icon("search", 14., c(MUTED)))
                    .cleanable(true),
            )
            .child(div().h(px(8.)))
            .child({
                // Only the rows in view are built: a thousand files would
                // otherwise lay out every frame.
                let items = std::rc::Rc::new(items);
                let review = self.shown();
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .child(
                        uniform_list(
                            "gl-tree",
                            items.len(),
                            cx.processor(move |v, range: std::ops::Range<usize>, _, cx| {
                                let Some(r) = review.clone() else { return vec![] };
                                let active = v.current_file();
                                range.map(|i| v.tree_row(&items[i], &r, active, cx)).collect::<Vec<_>>()
                            }),
                        )
                        .size_full()
                        .track_scroll(self.tree_scroll.clone()),
                    )
                    .when(none, |d| d.child(div().py_2().text_color(c(MUTED)).child("No files found.")))
            })
    }

    /// One row of the file tree: a folder, or a file with its counts.
    fn tree_row(&self, it: &TreeItem, r: &Review, active: Option<usize>, cx: &mut Context<Self>) -> AnyElement {
        let flat = self.tree_flat;
        match it {
            TreeItem::Folder { name, depth, path, folded } => {
                let (name, depth, path, folded) = (name.clone(), *depth, path.clone(), *folded);
                div()
                .id(SharedString::from(format!("dir-{path}")))
                .w_full()
                .flex()
                .items_center()
                .gap_1()
                .h(px(32.))
                .pl(px(4. + depth as f32 * 16.))
                .relative()
                .children(tree_guides(depth))
                .rounded(px(4.))
                .cursor_pointer()
                .hover(|s| s.bg(c(RAISED)))
                .text_color(c(TEXT))
                .child(app_icon(if folded { "chevron-right" } else { "chevron-down" }, 14., c(MUTED)))
                .child(app_icon("folder", 16., c(MUTED)))
                .child(div().ml_1().child(name))
                .on_click(cx.listener(move |v, _, _, cx| {
                    if !v.folded_dirs.remove(&path) {
                        v.folded_dirs.insert(path.clone());
                    }
                    cx.notify();
                }))
                .into_any_element()
            }
            TreeItem::File { index, name, depth } => {
                let (index, name, depth) = (*index, name.clone(), *depth);
                let f = &r.files[index];
                let (a, d) = f.counts();
                let (status_icon, status_color) = match f.status() {
                    "added" => ("square-plus", GREEN),
                    "deleted" => ("square-minus", RED),
                    _ => ("square-dot", ORANGE),
                };
                let viewed = self.loc.viewed.iter().any(|p| p == f.path());
                // List mode: the folder under the name.
                let dir = flat.then(|| f.path().rsplit_once('/').map(|(d, _)| d.to_string())).flatten();
                let name = div()
                    .flex()
                    .flex_col()
                    .child(name)
                    .when_some(dir, |d, dir| {
                        d.child(
                            div()
                                .truncate()
                                .text_size(px(12.))
                                .line_height(px(16.))
                                .font_weight(FontWeight::NORMAL)
                                .text_color(c(MUTED))
                                .child(dir),
                        )
                    });
                div()
                    .id(SharedString::from(format!("tree-{index}")))
                    .w_full()
                    .flex()
                    .items_center()
                    .gap_2()
                    // One height a mode: the tree is a uniform list.
                    .h(px(if flat { 48. } else { 32. }))
                    .pl(px(8. + depth as f32 * 16.))
                    .relative()
                    .children(tree_guides(depth))
                    .pr_2()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .when(active == Some(index), |d| d.bg(c(RAISED)))
                    .hover(|s| s.bg(c(RAISED)))
                    .child(file_icon(f.path(), 14.))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .font_weight(if viewed { FontWeight::NORMAL } else { FontWeight::SEMIBOLD })
                            .text_color(c(if viewed { MUTED } else { TEXT }))
                            .child(name),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .text_size(px(12.))
                            .child(div().text_color(c(GREEN)).child(format!("+{a}")))
                            .child(div().text_color(c(RED)).child(format!("-{d}"))),
                    )
                    .child(icon(status_icon, 14., c(status_color)))
                    .on_click(cx.listener(move |v, _, _, cx| v.scroll_to_file(index, cx)))
                    .into_any_element()
            }
        }
    }

    /// The file being read: the one gone to, else the one at the top.
    pub(crate) fn current_file(&self) -> Option<usize> {
        let n = self.shown().map_or(0, |r| r.files.len());
        self.current.filter(|&f| f < n).or(self.single_file).or_else(|| self.active_file())
    }

    /// The file whose card is at the top of the diff.
    pub(crate) fn active_file(&self) -> Option<usize> {
        if self.rows.is_empty() {
            return None;
        }
        let top = self.diff_list.logical_scroll_top().item_ix.min(self.rows.len() - 1);
        self.rows[..=top].iter().rev().find_map(|r| match r {
            Row::File(f) => Some(*f),
            _ => None,
        })
    }

    /// From the overview to a thread in the Changes tab: its file unfolded,
    /// a resolved one opened, and its line just above the top.
    pub(crate) fn show_thread(&mut self, id: &str, cx: &mut Context<Self>) {
        self.loc.section = crate::Section::Changes;
        // Threads are on the latest version's diff.
        if self.compare != crate::Compare::default() {
            self.set_compare(crate::Compare::default(), cx);
        }
        let Some(r) = self.review.ready().cloned() else { return };
        let Some((fi, ti)) = r
            .files
            .iter()
            .enumerate()
            .find_map(|(fi, f)| f.threads.iter().position(|(_, _, d)| d.id == id).map(|ti| (fi, ti)))
        else {
            cx.notify();
            return;
        };
        let f = &r.files[fi];
        self.folded.insert(f.path().to_string(), false);
        if self.single_file.is_some() {
            self.single_file = Some(fi);
        }
        if f.threads[ti].2.resolved() {
            self.open_resolved.insert(id.to_string());
        }
        self.rebuild_rows(cx);
        if let Some(i) = self.rows.iter().position(|row| *row == Row::Thread(fi, ti)) {
            self.reveal(i);
        }
        cx.notify();
    }

    /// Scroll so row `i` (a thread) shows with the line it's on visible
    /// below the pinned file header: start a few rows up, by their usual
    /// heights, until the header's height and the line's fit above.
    pub(crate) fn reveal(&mut self, i: usize) {
        const ROOM: f32 = 42. + CODE_LINE + 8.;
        let mut top = i;
        let mut above = 0.;
        while top > 0 && above < ROOM {
            top -= 1;
            above += match self.rows[top] {
                Row::File(_) => 42.,
                Row::Gap => 16.,
                Row::Thread(..) | Row::Message(_) => 100.,
                _ => CODE_LINE,
            };
            if matches!(self.rows[top], Row::File(_)) {
                break;
            }
        }
        self.diff_list.scroll_to(ListOffset { item_ix: top, offset_in_item: px(0.) });
    }

    /// GitLab's `j`/`k`: the next or previous file's header to the top.
    pub(crate) fn step_file(&mut self, forward: bool, cx: &mut Context<Self>) {
        let n = self.shown().map_or(0, |r| r.files.len());
        if n == 0 {
            return;
        }
        let at = self.current_file().unwrap_or(0);
        if self.single_file.is_some() {
            let next = if forward { (at + 1).min(n - 1) } else { at.saturating_sub(1) };
            self.scroll_to_file(next, cx);
            return;
        }
        // At a header already: step from it; mid-file going back: to its own header.
        let top = self.diff_list.logical_scroll_top();
        let at_header = self.current.is_some() || self.rows.get(top.item_ix) == Some(&Row::File(at)) && top.offset_in_item <= px(1.);
        let next = match forward {
            true => (at + 1).min(n - 1),
            false if at_header => at.saturating_sub(1),
            false => at,
        };
        self.scroll_to_file(next, cx);
    }

    /// GitLab's `n`/`p` and the arrows by the threads count: the next or
    /// previous unresolved thread, wrapping around, to the top (with the
    /// line it's on just above).
    pub(crate) fn step_thread(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(r) = self.shown() else { return };
        // One file at a time: step through every file's threads in order.
        if self.single_file.is_some() {
            let all: Vec<(usize, String)> = r
                .files
                .iter()
                .enumerate()
                .flat_map(|(fi, f)| f.threads.iter().filter(|(_, _, d)| !d.resolved()).map(move |(_, _, d)| (fi, d.id.clone())))
                .collect();
            if all.is_empty() {
                return;
            }
            let here = self.thread_cursor.and_then(|c| match self.rows.get(c) {
                Some(Row::Thread(f, t)) => all.iter().position(|(_, id)| *id == r.files[*f].threads[*t].2.id),
                _ => None,
            });
            let next = match (here, forward) {
                (Some(i), true) => (i + 1) % all.len(),
                (Some(i), false) => (i + all.len() - 1) % all.len(),
                (None, true) => 0,
                (None, false) => all.len() - 1,
            };
            let id = all[next].1.clone();
            self.show_thread(&id, cx);
            self.thread_cursor = self.rows.iter().position(|row| match row {
                Row::Thread(f, t) => r.files[*f].threads[*t].2.id == id,
                _ => false,
            });
            return;
        }
        // Unresolved threads in folded files: unfold those first.
        let mut unfold = vec![];
        for f in &r.files {
            if f.threads.iter().any(|(_, _, d)| !d.resolved()) && self.is_folded(f) {
                unfold.push(f.path().to_string());
            }
        }
        if !unfold.is_empty() {
            for p in unfold {
                self.folded.insert(p, false);
            }
            self.rebuild_rows(cx);
        }
        let threads: Vec<usize> = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| matches!(row, Row::Thread(f, t) if !r.files[*f].threads[*t].2.resolved()))
            .map(|(i, _)| i)
            .collect();
        if threads.is_empty() {
            return;
        }
        // From the last thread stepped to, while it's still in view; else
        // from what's at the top.
        let top = self.diff_list.logical_scroll_top().item_ix;
        let at = self
            .thread_cursor
            .filter(|&c| self.rows.get(c).is_some_and(|r| matches!(r, Row::Thread(..))) && c >= top && c <= top + 40)
            .unwrap_or(top);
        let target = if forward {
            threads.iter().find(|&&i| i > at).or(threads.first())
        } else {
            threads.iter().rev().find(|&&i| i < at).or(threads.last())
        };
        if let Some(&i) = target {
            self.thread_cursor = Some(i);
            self.reveal(i);
        }
        cx.notify();
    }

    pub(crate) fn scroll_to_file(&mut self, file: usize, cx: &mut Context<Self>) {
        // One file at a time: show that one.
        if self.single_file.is_some_and(|f| f != file) {
            self.single_file = Some(file);
            self.rebuild_rows(cx);
        }
        if let Some(ix) = self.rows.iter().position(|r| *r == Row::File(file)) {
            self.diff_list.scroll_to(ListOffset { item_ix: ix, offset_in_item: px(0.) });
        }
        // Near the end the list can't put the file at the top yet: a file still
        // loading is short. Aim at it again as rows come in, for a moment.
        self.jump = Some((file, std::time::Instant::now()));
        self.current = Some(file);
        cx.notify();
    }

    fn row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.rows.get(ix).copied() else {
            return div().into_any_element();
        };
        let Some(r) = self.shown() else {
            return div().into_any_element();
        };
        let mono = self.mono(cx);
        match row {
            Row::File(fi) => self.file_header(&r.files[fi], fi, cx).into_any_element(),
            Row::Message(fi) if matches!(r.files[fi].body(), Body::Binary) && is_image(r.files[fi].path()) => {
                // GitLab's "2-up": old on the left in red, new on the right in green.
                let side = |s: &Shot, border: Option<u32>| {
                    let picture = match &s.image {
                        Some(i) => img(i.clone()).max_w(px(400.)).max_h(px(400.)).into_any_element(),
                        // GitLab's broken image.
                        None => div()
                            .size(px(20.))
                            .border_1()
                            .border_color(c(BORDER))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon("file", 14., c(MUTED)))
                            .into_any_element(),
                    };
                    let mut caption = div()
                        .flex()
                        .gap_1()
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .text_color(c(MUTED))
                        .child(human_size(s.bytes));
                    if let Some((w, h)) = s.size {
                        caption = caption
                            .child("|")
                            .child(div().font_weight(FontWeight::SEMIBOLD).child("W:"))
                            .child(w.to_string())
                            .child("|")
                            .child(div().font_weight(FontWeight::SEMIBOLD).child("H:"))
                            .child(h.to_string());
                    }
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_1()
                        .child(div().when_some(border, |d, b| d.border_1().border_color(c(b))).child(picture))
                        .child(caption)
                };
                let body = match self.pictures.get(&fi) {
                    Some(Load::Ready(p)) => match (&p.old, &p.new) {
                        (Some(o), Some(n)) => div()
                            .flex()
                            .w_full()
                            .justify_around()
                            .child(side(o, Some(0xe5484d)))
                            .child(side(n, Some(GREEN))),
                        (Some(s), None) | (None, Some(s)) => side(s, None),
                        (None, None) => div(),
                    },
                    Some(Load::Failed(e)) => div().text_color(c(RED)).child(e.text.clone()),
                    _ => spinner("gl-picture-loading").py_0(),
                };
                card(div().flex().justify_center().px_4().py_4().bg(c(CARD)).child(body)).into_any_element()
            }
            Row::Message(fi) if matches!(r.files[fi].body(), Body::Pending) => {
                let path = r.files[fi].path().to_string();
                let failed = match self.late.get(&path) {
                    None => {
                        self.fetch_late(fi, cx);
                        None
                    }
                    Some(e) => e.clone(),
                };
                let body = match failed {
                    Some(e) => div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().text_color(c(RED)).child(format!("Couldn't load the changes: {e}")))
                        .child(
                            div()
                                .id(SharedString::from(format!("late-retry-{fi}")))
                                .text_color(c(LINK))
                                .cursor_pointer()
                                .hover(|s| s.underline())
                                .child("Retry")
                                .on_click(cx.listener(move |v, _, _, cx| {
                                    v.late.remove(&path);
                                    v.fetch_late(fi, cx);
                                })),
                        ),
                    None => div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_color(c(MUTED))
                        .child(spinner("gl-late").py_0())
                        .child("Loading…"),
                };
                card(div().px_4().py_3().bg(c(CARD)).child(body)).into_any_element()
            }
            Row::Message(fi) => {
                let f = &r.files[fi];
                let text = match f.body() {
                    Body::Binary => "Binary file not shown.",
                    Body::Unchanged if f.meta.renamed_file => "File moved.",
                    Body::Unchanged => "File mode changed.",
                    Body::TooLarge => "Changes are too large to be shown.",
                    Body::Collapsed => "This file is collapsed because it's large.",
                    Body::Text(_) | Body::Pending => "",
                };
                // Where GitLab can't show it here, its file page can.
                let blob = matches!(f.body(), Body::TooLarge | Body::Collapsed | Body::Unchanged)
                    .then(|| {
                        let project = r.mr.web_url.split("/-/").next().filter(|p| !p.is_empty())?;
                        let sha = r.mr.diff_refs.as_ref()?.head_sha.clone();
                        Some(format!("{project}/-/blob/{sha}/{}", f.meta.new_path))
                    })
                    .flatten()
                    .filter(|_| !f.meta.deleted_file);
                card(
                    div()
                        .flex()
                        .flex_col()
                        .items_start()
                        .gap_2()
                        .px_4()
                        .py_3()
                        .bg(c(CARD))
                        .text_color(c(TEXT))
                        .child(text)
                        .when_some(blob, |d, url| {
                            d.child(
                                div()
                                    .id(SharedString::from(format!("view-file-{fi}")))
                                    .flex()
                                    .items_center()
                                    .h(px(32.))
                                    .px_3()
                                    .rounded(px(8.))
                                    .bg(ca(0x89888d66))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(ca(0x89888d99)))
                                    .child("View file")
                                    .on_click(move |_, window, cx| abromux_plugin::open_url(&url, window, cx)),
                            )
                        }),
                )
                .into_any_element()
            }
            Row::Hunk(fi, hi, hidden) => {
                let h = &r.files[fi].body().hunks()[hi];
                let expandable = !r.files[fi].meta.new_file && !r.files[fi].meta.deleted_file;
                card(
                    code(div(), mono)
                        .flex()
                        .bg(c(HUNK_BG))
                        .text_color(c(HUNK_TEXT))
                        .child(
                            div()
                                .w(px(NUM_W * 2.))
                                .flex_none()
                                .flex()
                                .when(expandable, |d| d.children(self.expand_buttons(fi, Some(hi), hidden, cx))),
                        )
                        .child(div().pl(px(18.75)).flex_1().min_w(px(0.)).truncate().child(h.header.clone())),
                )
                .into_any_element()
            }
            Row::Context(fi, new, old) => {
                let text = self.contents.get(&fi).and_then(|l| l.ready().cloned());
                let i = new as usize - 1;
                let line = crate::diff::Line {
                    kind: crate::diff::Kind::Context,
                    old: Some(old),
                    new: Some(new),
                    text: text.as_ref().and_then(|t| t.lines.get(i).cloned()).unwrap_or_default(),
                    spans: text.as_ref().and_then(|t| t.spans.get(i).cloned()).unwrap_or_default(),
                    changed: vec![],
                };
                let (t, h) = line_highlights(&line);
                if self.loc.parallel {
                    let (left, right) = (self.sel_text(t.clone(), h.clone(), Group::Diff(0), ix, cx), self.sel_text(t, h, Group::Diff(1), ix, cx));
                    let row = code(div(), mono)
                        .flex()
                        .w_full()
                        .child(half_with(Some(&line), false, Some(left)))
                        .child(half_with(Some(&line), true, Some(right)));
                    return card(row).into_any_element();
                }
                let text = self.sel_text(t, h, Group::Diff(0), ix, cx);
                card(code_line_with(&line, &mono, None, false, text)).into_any_element()
            }
            Row::Pair(fi, hi, l, rt) => {
                let f = &r.files[fi];
                let lines = &f.body().hunks()[hi].lines;
                // Folded resolved threads on either line: an avatar to open them.
                let folded: Vec<_> = f
                    .threads
                    .iter()
                    .filter(|(h, x, d)| {
                        *h == hi && (Some(*x) == l || Some(*x) == rt) && d.resolved() && !self.open_resolved.contains(&d.id)
                    })
                    .collect();
                let gutter = folded.first().map(|(_, _, d)| {
                    let id = d.id.clone();
                    div()
                        .id(SharedString::from(format!("gutter-{id}")))
                        .absolute()
                        .left(px(2.))
                        .top(px(2.))
                        .cursor_pointer()
                        .child(self.avatars.of(&d.notes[0].author, 16.))
                        .on_click(cx.listener(move |v, _, _, cx| {
                            v.open_resolved.insert(id.clone());
                            v.rebuild_rows(cx);
                        }))
                });
                // Each side selects on its own, as in GitLab.
                let mut side = |i: Option<usize>, s: u8| {
                    let (t, h) = line_highlights(&lines[i?]);
                    Some(self.sel_text(t, h, Group::Diff(s), ix, cx))
                };
                let (left, right) = (side(l, 0), side(rt, 1));
                let row = code(div(), mono)
                    .relative()
                    .flex()
                    .w_full()
                    .child(half_with(l.map(|i| &lines[i]), false, left))
                    .child(half_with(rt.map(|i| &lines[i]), true, right))
                    .children(gutter);
                // The bubble goes over the new side's number, else the old side's.
                let Some(t) = rt.or(l).map(|i| &lines[i]) else {
                    return card(row).into_any_element();
                };
                let key = (fi, t.old, t.new);
                let button = self.comment_button(key, cx).map(|b| match rt {
                    Some(_) => b.left(relative(0.5)).ml(px(4.)),
                    None => b.left(px(NUM_W - 22.)),
                });
                card(div().child(self.hover_line(key, row.children(button), cx))).into_any_element()
            }
            Row::Expand(fi) => {
                let hidden = self.contents.get(&fi).and_then(|l| l.ready()).map(|t| t.lines.len() as u32);
                card(
                    code(div(), mono)
                        .flex()
                        .h(px(CODE_LINE))
                        .bg(c(HUNK_BG))
                        .child(div().w(px(NUM_W * 2.)).flex_none().flex().children(self.expand_buttons(fi, None, hidden.unwrap_or(u32::MAX), cx))),
                )
                .into_any_element()
            }
            Row::Line(fi, hi, li) => {
                let f = &r.files[fi];
                let l = &f.body().hunks()[hi].lines[li];
                // Folded resolved threads on this line: their avatars in the gutter.
                let folded: Vec<_> = f
                    .threads
                    .iter()
                    .filter(|(h, l2, d)| (*h, *l2) == (hi, li) && d.resolved() && !self.open_resolved.contains(&d.id))
                    .collect();
                let gutter = (!folded.is_empty()).then(|| {
                    let id = folded[0].2.id.clone();
                    div()
                        .id(SharedString::from(format!("gutter-{id}")))
                        .absolute()
                        .left(px(2.))
                        .top(px(2.))
                        .cursor_pointer()
                        .child(self.avatars.of(&folded[0].2.notes[0].author, 16.))
                        .on_click(cx.listener(move |v, _, _, cx| {
                            v.open_resolved.insert(id.clone());
                            v.rebuild_rows(cx);
                        }))
                        .into_any_element()
                });
                // The number cells select the line, as GitLab's line links do.
                let key = (fi, l.old, l.new);
                let selected = self.selected_line == Some(key);
                let pick = div()
                    .id(SharedString::from(format!("num-{fi}-{hi}-{li}")))
                    .absolute()
                    .top_0()
                    .left_0()
                    .w(px(NUM_W * 2.))
                    .h_full()
                    .cursor_pointer()
                    .on_click(cx.listener(move |v, _, _, cx| {
                        v.selected_line = if v.selected_line == Some(key) { None } else { Some(key) };
                        cx.notify();
                    }))
                    .into_any_element();
                // The avatar stays on top of the click area.
                // Over the number cells: absolute children go by their parent.
                let gutter = div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .children([Some(pick), gutter].into_iter().flatten())
                    .children(self.comment_button(key, cx).map(|b| b.left(px(NUM_W - 22.))))
                    .into_any_element();
                let (t, h) = line_highlights(l);
                let text = self.sel_text(t, h, Group::Diff(0), ix, cx);
                let row = code_line_with(l, &mono, Some(gutter), selected, text);
                card(div().child(self.hover_line(key, row, cx))).into_any_element()
            }
            Row::Thread(fi, ti) => {
                let f = &r.files[fi];
                let d = &f.threads[ti].2;
                let path = f.meta.new_path.clone();
                let resolved = d.resolved();
                let id = d.id.clone();
                // Side by side, a thread sits under its side's half: the old
                // side for a removed line, else the new one (GitLab's way).
                let old_side = d.position().is_some_and(|p| p.new_line.is_none());
                let parallel = self.loc.parallel;
                let thread = div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .when(parallel, |d| d.border_l_1().border_r_1().border_color(c(BORDER)))
                    .children(self.thread_notes(d, Some(&path), true, true, window, cx))
                    .child(self.reply_bar(&d.id, d.resolvable(), d.resolved(), cx))
                    // GitLab's "Start another thread" on the same line, while
                    // none is being written there.
                    .when_some(
                        d.position()
                            .map(|p| (fi, p.old_line, p.new_line))
                            .filter(|k| self.me.is_some() && self.commenting != Some(*k)),
                        |el, key| {
                            el.child(
                                div().flex().px_4().py_3().border_t_1().border_color(c(RAISED)).bg(c(CARD)).child(
                                    crate::edit::secondary(SharedString::from(format!("another-{}", d.id)), "Start another thread")
                                        .on_click(cx.listener(move |v, _, window, cx| v.open_line_comment(key, window, cx))),
                                ),
                            )
                        },
                    );
                let empty =|| div().flex_1().bg(c(EMPTY_BG));
                let row = if parallel {
                    if old_side { div().flex().child(thread).child(empty()) } else { div().flex().child(empty()).child(thread) }
                } else {
                    div().flex().child(thread)
                };
                card(
                    div()
                        .bg(c(BG))
                        .border_t_1()
                        .border_b_1()
                        .border_color(c(BORDER))
                        .flex()
                        .flex_col()
                        .child(row)
                        .when(resolved, |el| {
                            el.child(
                                div()
                                    .id(SharedString::from(format!("fold-{id}")))
                                    .px_4()
                                    .py_2()
                                    .border_t_1()
                                    .border_color(c(RAISED))
                                    .cursor_pointer()
                                    .text_color(c(LINK))
                                    .child(match d.resolution() {
                                        Some((who, _)) => format!("Resolved by {} · Hide thread", who.name),
                                        None => "Hide thread".into(),
                                    })
                                    .on_click(cx.listener(move |v, _, _, cx| {
                                        v.open_resolved.remove(&id);
                                        v.rebuild_rows(cx);
                                    })),
                            )
                        }),
                )
                .into_any_element()
            }
            Row::End(_) => card(div().h(px(1.)).border_b_1().border_color(c(BORDER)).rounded_b(px(8.))).into_any_element(),
            Row::Draft(_) => {
                let form = self.line_form(cx);
                if !self.loc.parallel {
                    return card(form).into_any_element();
                }
                // Side by side, under its side's half, as threads are.
                let old_side = self.commenting.is_some_and(|(_, _, new)| new.is_none());
                let form = div().flex_1().min_w(px(0.)).border_l_1().border_r_1().border_color(c(BORDER)).child(form);
                let empty = div().flex_1().bg(c(EMPTY_BG));
                let row = if old_side { div().flex().child(form).child(empty) } else { div().flex().child(empty).child(form) };
                card(row).into_any_element()
            }
            Row::Gap => div().h(px(16.)).into_any_element(),
        }
    }

    fn file_header(&self, f: &File, fi: usize, cx: &mut Context<Self>) -> Div {
        let (a, d) = f.counts();
        let folded = self.is_folded(f);
        let viewed = self.loc.viewed.iter().any(|p| p == f.path());
        let current = self.current_drawn.get() == Some(fi);
        let path = f.path().to_string();
        let mode = (f.meta.a_mode != f.meta.b_mode && !f.meta.a_mode.is_empty())
            .then(|| format!("{} → {}", f.meta.a_mode, f.meta.b_mode));
        let name: AnyElement = if f.meta.renamed_file {
            let (o, n) = changed_ranges(&f.meta.old_path, &f.meta.new_path);
            let piece = |s: &str, r: Option<std::ops::Range<usize>>, bg: u32| {
                let mut hl = vec![];
                if let Some(r) = r {
                    hl.push((r, HighlightStyle { background_color: Some(c(bg)), ..Default::default() }));
                }
                StyledText::new(SharedString::from(s.to_string())).with_highlights(hl)
            };
            div()
                .flex()
                .gap_1()
                .child(piece(&f.meta.old_path, o, 0xac203a))
                .child("→")
                .child(piece(&f.meta.new_path, n, 0x0b645a))
                .into_any_element()
        } else {
            div().child(path.clone()).into_any_element()
        };
        let toggle_path = path.clone();
        let viewed_path = path.clone();
        let copy_path = path.clone();
        div()
            .w_full()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .bg(c(CARD))
            .border_1()
            .border_color(c(if current { FOCUS } else { BORDER }))
            .rounded_t(px(8.))
            .when(folded, |d| d.rounded_b(px(8.)))
            .text_color(c(TEXT))
            .child(
                div()
                    .id(SharedString::from(format!("fold-file-{fi}")))
                    .cursor_pointer()
                    .child(app_icon(if folded { "chevron-right" } else { "chevron-down" }, 16., c(TEXT)))
                    .on_click(cx.listener(move |v, _, _, cx| v.toggle_folded(&toggle_path, cx))),
            )
            .child(div().font_weight(FontWeight::SEMIBOLD).child(name))
            .when(f.meta.deleted_file, |d| d.child("deleted"))
            .when(f.meta.generated_file, |d| d.child(badge("Generated", c(0xa4a3a8), c(BG))))
            .child(
                div()
                    .id(SharedString::from(format!("copy-{fi}")))
                    .cursor_pointer()
                    .child(icon("copy", 14., c(MUTED)))
                    .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copy_path.clone()))),
            )
            .children(mode.map(|m| div().text_size(px(12.)).child(m)))
            .child(div().flex_1())
            .map(|el| match self.pictures.get(&fi).and_then(|p| p.ready()).and_then(Picture::size_change) {
                // An image: how much its size changed, and its size.
                Some((delta, now)) => el
                    .child(div().text_color(c(if delta < 0 { RED } else { GREEN })).child(format!(
                        "{}{}",
                        if delta < 0 { "−" } else { "+" },
                        human_size(delta.unsigned_abs() as usize)
                    )))
                    .child(div().text_color(c(TEXT)).child(format!("({})", human_size(now)))),
                None => el
                    .child(div().text_color(c(GREEN)).child(format!("+{a}")))
                    .child(div().text_color(c(RED)).child(format!("−{d}"))),
            })
            .child(
                div()
                    .id(SharedString::from(format!("viewed-{fi}")))
                    .flex()
                    .items_center()
                    .gap_2()
                    .ml_2()
                    .px_1()
                    .rounded(px(4.))
                    .border_2()
                    // `v` marks the file being read: its Viewed is ringed.
                    .border_color(if current { c(FOCUS) } else { gpui::transparent_black() })
                    .cursor_pointer()
                    .child(checkbox(viewed))
                    .child("Viewed")
                    .on_click(cx.listener(move |v, _, _, cx| v.toggle_viewed(&viewed_path, cx))),
            )
            .when(!f.threads.is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .ml_2()
                        .text_color(c(MUTED))
                        .child(icon("comment", 16., c(MUTED)))
                        .child(f.threads.len().to_string()),
                )
            })
            // GitLab's "View file @ <sha>", from the header's menu.
            .when_some(self.blob_url(f), |d, (url, short)| {
                let tip: SharedString = format!("View file @ {short}").into();
                d.child(
                    div()
                        .id(SharedString::from(format!("view-{fi}")))
                        .ml_2()
                        .size(px(28.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .hover(|s| s.bg(ca(0x89888d66)))
                        .child(icon("external-link", 14., c(TEXT)))
                        .tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
                        .on_click(move |_, window, cx| abromux_plugin::open_url(&url, window, cx)),
                )
            })
    }

    /// The file's page in GitLab at the shown head commit, and that commit's
    /// short sha. None for a deleted file.
    fn blob_url(&self, f: &File) -> Option<(String, String)> {
        let r = self.shown()?;
        if f.meta.deleted_file {
            return None;
        }
        let project = r.mr.web_url.split("/-/").next().filter(|p| !p.is_empty())?.to_string();
        let sha = r.mr.diff_refs.as_ref()?.head_sha.clone();
        let short = sha.get(..8).unwrap_or(&sha).to_string();
        Some((format!("{project}/-/blob/{sha}/{}", f.meta.new_path), short))
    }

    /// A diff line that tells the view when the pointer is on it, so only
    /// that line has the comment bubble (one per line costs scrolling time).
    fn hover_line(&self, key: crate::edit::LineKey, row: Div, cx: &mut Context<Self>) -> Stateful<Div> {
        row.id(SharedString::from(format!("line-{}-{:?}-{:?}", key.0, key.1, key.2)))
            .on_hover(cx.listener(move |v, on: &bool, _, cx| {
                if *on {
                    v.hovered_line = Some(key);
                } else if v.hovered_line == Some(key) {
                    v.hovered_line = None;
                }
                cx.notify();
            }))
    }

    /// GitLab's speech bubble on the hovered line: a new thread there. Only
    /// on the latest version, where GitLab starts threads; to be placed.
    fn comment_button(&self, key: crate::edit::LineKey, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let latest = self.compare == crate::Compare::default();
        if !latest || self.loc.iid.is_none() || self.hovered_line != Some(key) {
            return None;
        }
        Some(div()
            .id(SharedString::from(format!("comment-{}-{:?}-{:?}", key.0, key.1, key.2)))
            .absolute()
            .top(px(1.))
            .size(px(18.))
            .rounded(px(4.))
            .bg(c(0x1068bf))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .child(icon("comment", 12., c(HEADING)))
            .tooltip(hint("Add a comment to this line"))
            .on_click(cx.listener(move |v, _, window, cx| {
                // Not also the number's click under it (selecting the line).
                cx.stop_propagation();
                if v.compare == crate::Compare::default() {
                    v.open_line_comment(key, window, cx)
                }
            })))
    }

    /// GitLab's expand buttons in a hunk's (or the end's) number gutter:
    /// 20 more lines, or all of them.
    fn expand_buttons(&self, fi: usize, hunk: Option<usize>, hidden: u32, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let button = |id: String, icon_name: &'static str, n: u32, cx: &mut Context<Self>| {
            div()
                .id(SharedString::from(id))
                .flex_1()
                .h(px(CODE_LINE))
                .flex()
                .items_center()
                .justify_center()
                .bg(c(0x646163))
                .cursor_pointer()
                .hover(|s| s.bg(c(0x7a777a)))
                .child(icon(icon_name, 14., c(TEXT)))
                .on_click(cx.listener(move |v, _, _, cx| v.expand(fi, hunk, n, cx)))
                .into_any_element()
        };
        let key = format!("{fi}-{}", hunk.map_or("end".into(), |h| h.to_string()));
        let more = if hunk.is_some() { "expand-up" } else { "expand-down" };
        if hidden > 20 {
            vec![
                button(format!("more-{key}"), more, 20, cx),
                button(format!("all-{key}"), "expand-all", u32::MAX, cx),
            ]
        } else {
            vec![button(format!("all-{key}"), "expand-all", u32::MAX, cx)]
        }
    }

    /// Show `n` more unchanged lines above hunk `hunk`, or after the last
    /// one; the file's text is fetched the first time.
    pub(crate) fn expand(&mut self, fi: usize, hunk: Option<usize>, n: u32, cx: &mut Context<Self>) {
        let slot = match hunk {
            Some(h) => self.above.entry((fi, h)).or_insert(0),
            None => self.below.entry(fi).or_insert(0),
        };
        *slot = slot.saturating_add(n);
        if !self.contents.contains_key(&fi) {
            self.fetch_text(fi, cx);
        }
        self.rebuild_rows(cx);
    }

    /// Fetch the images among the binary files, for their previews.
    pub(crate) fn fetch_pictures(&mut self, cx: &mut Context<Self>) {
        let (Some(r), Some(c)) = (self.shown(), self.client.clone()) else { return };
        let Some(refs) = r.mr.diff_refs.clone() else { return };
        for (fi, f) in r.files.iter().enumerate() {
            if !matches!(f.body(), Body::Binary) || !is_image(f.path()) || self.pictures.contains_key(&fi) {
                continue;
            }
            // The old version from the base, the new one from the head.
            let old = (!f.meta.new_file).then(|| (f.meta.old_path.clone(), refs.base_sha.clone()));
            let new = (!f.meta.deleted_file).then(|| (f.meta.new_path.clone(), refs.head_sha.clone()));
            let (c, project) = (c.clone(), r.mr.project_id);
            self.pictures.insert(fi, Load::Loading);
            cx.spawn(async move |this, cx| {
                let got = cx
                    .background_spawn(async move {
                        let shot = |side: Option<(String, String)>| -> anyhow::Result<Option<Shot>> {
                            let Some((path, sha)) = side else { return Ok(None) };
                            Ok(Some(Shot::of(c.raw_bytes(project, &path, &sha)?)))
                        };
                        anyhow::Ok(Picture { old: shot(old)?, new: shot(new)? })
                    })
                    .await;
                this.update(cx, |v, cx| {
                    v.pictures.insert(fi, got.map_or_else(|e| Load::Failed(Failure::of(&e)), Load::Ready));
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    /// A file GitLab left out: both sides from the repository, diffed here,
    /// in the background. A few at a time, so scrolling through a thousand
    /// of them doesn't fire a thousand requests at once.
    pub(crate) fn fetch_late(&mut self, fi: usize, cx: &mut Context<Self>) {
        const AT_ONCE: usize = 6;
        let (Some(r), Some(c)) = (self.review.ready().cloned(), self.client.clone()) else { return };
        let Some(f) = r.files.get(fi) else { return };
        let path = f.path().to_string();
        if self.late.contains_key(&path) || f.late.get().is_some() {
            return;
        }
        if self.late.values().filter(|e| e.is_none()).count() >= AT_ONCE {
            // Asked again when it's drawn after one of these lands.
            return;
        }
        let Some(refs) = r.mr.diff_refs.clone() else { return };
        self.late.insert(path.clone(), None);
        let (project, old_path, new_path) = (r.mr.project_id, f.meta.old_path.clone(), f.meta.new_path.clone());
        let (new_file, deleted) = (f.meta.new_file, f.meta.deleted_file);
        let slot = f.late.clone();
        cx.spawn(async move |this, cx| {
            let got = cx
                .background_spawn(async move {
                    let old = if new_file { None } else { c.raw_maybe(project, &old_path, &refs.base_sha)? };
                    let new = if deleted { None } else { c.raw_maybe(project, &new_path, &refs.head_sha)? };
                    let status = match (&old, &new) {
                        (None, Some(_)) => Some("added"),
                        (Some(_), None) => Some("deleted"),
                        _ => None,
                    };
                    let body = crate::diff::Body::local(old.as_deref(), new.as_deref(), &new_path);
                    let _ = slot.set(crate::review::Late { body, status });
                    anyhow::Ok(())
                })
                .await;
            this.update(cx, |v, cx| {
                match got {
                    Ok(()) => {
                        v.late.remove(&path);
                    }
                    Err(e) => {
                        v.late.insert(path, Some(format!("{e:#}")));
                    }
                }
                v.rebuild_rows(cx);
            })
            .ok();
        })
        .detach();
    }

    fn fetch_text(&mut self, fi: usize, cx: &mut Context<Self>) {
        let (Some(r), Some(c)) = (self.shown(), self.client.clone()) else { return };
        let Some(sha) = r.mr.diff_refs.as_ref().map(|d| d.head_sha.clone()) else { return };
        let (project, path) = (r.mr.project_id, r.files[fi].meta.new_path.clone());
        self.contents.insert(fi, Load::Loading);
        cx.spawn(async move |this, cx| {
            let got = cx
                .background_spawn(async move {
                    let text = c.raw_file(project, &path, &sha)?;
                    let mut hl = crate::highlight::Highlighter::new(&path);
                    let lines: Vec<String> = text.lines().map(str::to_string).collect();
                    let spans = lines.iter().map(|l| hl.line(l)).collect();
                    anyhow::Ok(FileText { lines, spans })
                })
                .await;
            this.update(cx, |v, cx| {
                v.contents.insert(fi, got.map_or_else(|e| Load::Failed(Failure::of(&e)), |t| Load::Ready(std::rc::Rc::new(t))));
                v.rebuild_rows(cx);
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn is_folded(&self, f: &File) -> bool {
        let path = f.path();
        match self.folded.get(path) {
            Some(folded) => *folded,
            // Viewed and generated files fold, as in GitLab.
            None => f.meta.generated_file || self.loc.viewed.iter().any(|p| p == path),
        }
    }

    pub(crate) fn toggle_folded(&mut self, path: &str, cx: &mut Context<Self>) {
        let r = self.shown();
        let now = r.as_ref().and_then(|r| r.files.iter().find(|f| f.path() == path)).map(|f| self.is_folded(f));
        if let Some(now) = now {
            self.folded.insert(path.to_string(), !now);
            self.rebuild_rows(cx);
        }
    }

    /// `v`: the file being read is Viewed (it folds), and the next file not
    /// viewed yet comes to the top, highlighted with its Viewed ringed.
    /// Unmarking is the checkbox's.
    pub(crate) fn mark_viewed_here(&mut self, cx: &mut Context<Self>) {
        let Some(r) = self.shown() else { return };
        let Some(fi) = self.current_file() else { return };
        let Some(path) = r.files.get(fi).map(|f| f.path().to_string()) else { return };
        if !self.loc.viewed.contains(&path) {
            self.toggle_viewed(&path, cx);
        }
        let n = r.files.len();
        let next = (1..n).map(|d| (fi + d) % n).find(|&i| !self.loc.viewed.iter().any(|p| p == r.files[i].path()));
        // All viewed: stay on this one.
        self.scroll_to_file(next.unwrap_or(fi), cx);
    }

    pub(crate) fn toggle_viewed(&mut self, path: &str, cx: &mut Context<Self>) {
        if let Some(i) = self.loc.viewed.iter().position(|p| p == path) {
            self.loc.viewed.remove(i);
        } else {
            self.loc.viewed.push(path.to_string());
        }
        // Viewed folds the file, unviewed unfolds it.
        self.folded.remove(path);
        self.rebuild_rows(cx);
    }

    /// Recompute the rows after folding or opening something, keeping the
    /// row at the top of the view where it is.
    pub(crate) fn rebuild_rows(&mut self, cx: &mut Context<Self>) {
        let Some(r) = self.shown() else {
            self.rows.clear();
            self.diff_list.reset(0);
            cx.notify();
            return;
        };
        let top = self.diff_list.logical_scroll_top();
        let anchor = self.rows.get(top.item_ix).copied();
        let shape = Shape {
            folded: &|f: &File| self.is_folded(f),
            open: &|id: &str| self.open_resolved.contains(id),
            above: &|f, h| self.above.get(&(f, h)).copied().unwrap_or(0),
            below: &|f| self.below.get(&f).copied().unwrap_or(0),
            lines: &|f| self.contents.get(&f).and_then(|t| t.ready()).map(|t| t.lines.len() as u32),
            parallel: self.loc.parallel,
            draft: self.commenting,
        };
        let mut rows = rows(&r, &shape);
        // GitLab's "Show one file at a time": only the chosen file's rows.
        if let Some(only) = self.single_file {
            let only = only.min(r.files.len().saturating_sub(1));
            let mut last = None;
            rows.retain(|row| {
                let f = row.file().or(last);
                if row.file().is_some() {
                    last = row.file();
                }
                f == Some(only)
            });
        }
        // The same row, though a hunk's count of hidden lines may change.
        let same = |a: &Row, b: &Row| match (a, b) {
            (Row::Hunk(f, h, _), Row::Hunk(g, k, _)) => (f, h) == (g, k),
            _ => a == b,
        };
        let new_top = anchor.and_then(|a| {
            rows.iter().position(|r| same(r, &a)).or_else(|| {
                let file = match a {
                    Row::File(f)
                    | Row::Message(f)
                    | Row::Hunk(f, _, _)
                    | Row::Line(f, _, _)
                    | Row::Pair(f, _, _, _)
                    | Row::Context(f, _, _)
                    | Row::Expand(f)
                    | Row::Thread(f, _)
                    | Row::End(f)
                    | Row::Draft(f) => f,
                    Row::Gap => return None,
                };
                rows.iter().position(|r| *r == Row::File(file))
            })
        });
        self.rows = rows;
        self.diff_list.reset(self.rows.len());
        let jump = self.jump.filter(|(_, at)| at.elapsed() < std::time::Duration::from_secs(5)).map(|(f, _)| f);
        let new_top = match jump.and_then(|f| self.rows.iter().position(|r| *r == Row::File(f))) {
            Some(ix) => {
                self.diff_list.scroll_to(ListOffset { item_ix: ix, offset_in_item: px(0.) });
                cx.notify();
                return;
            }
            None => new_top,
        };
        if let Some(ix) = new_top {
            let kept = anchor.is_some_and(|a| same(&self.rows[ix], &a));
            let offset = if kept { top.offset_in_item } else { px(0.) };
            self.diff_list.scroll_to(ListOffset { item_ix: ix, offset_in_item: offset });
        }
        cx.notify();
    }
}

/// GitLab's thin lines down the tree, one under each enclosing folder's
/// chevron.
fn tree_guides(depth: usize) -> impl Iterator<Item = Div> {
    (0..depth).map(|k| div().absolute().top_0().bottom_0().left(px(11. + k as f32 * 16.)).w(px(1.)).bg(c(RAISED)))
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui's `test` macro would shadow `#[test]`.
    use super::{Row, Shape, TreeItem, clamp_tree_width, rows, tree, tree_folded};
    use gpui::px;

    #[test]
    fn tree_width_leaves_the_diff_room() {
        assert_eq!(clamp_tree_width(px(500.), px(1400.)), px(500.));
        assert_eq!(clamp_tree_width(px(50.), px(1400.)), px(200.), "not narrower than 200");
        assert_eq!(clamp_tree_width(px(1300.), px(1400.)), px(1000.), "400 left for the diff");
        assert_eq!(clamp_tree_width(px(500.), px(500.)), px(200.), "a narrow tab: the least");
    }
    use crate::{review::Review, types::*};

    fn review() -> Review {
        let diff = |path: &str, d: &str| FileDiff {
            old_path: path.into(),
            new_path: path.into(),
            diff: d.into(),
            ..Default::default()
        };
        let thread = |id: &str, path: &str, new_line: u32, resolved: bool| Discussion {
            id: id.into(),
            notes: vec![Note {
                resolvable: true,
                resolved,
                position: Some(Position { new_path: Some(path.into()), new_line: Some(new_line), ..Default::default() }),
                ..Default::default()
            }],
            ..Default::default()
        };
        Review::build(
            MergeRequest::default(),
            vec![
                FileDiff { new_file: true, ..diff("src/b.rs", "@@ -0,0 +1,2 @@\n+x\n+y\n") },
                diff("a/README.md", "@@ -1 +1 @@\n-a\n+b\n"),
                diff("z.txt", ""),
            ],
            vec![thread("t1", "src/b.rs", 1, false), thread("t2", "src/b.rs", 2, true)],
            Approvals::default(),
            vec![],
        )
    }

    fn plain(r: &Review) -> Vec<Row> {
        rows(r, &Shape { folded: &|_| false, open: &|_| false, above: &|_, _| 0, below: &|_| 0, lines: &|_| None, draft: None, parallel: false })
    }

    #[test]
    fn rows_put_threads_under_lines_and_fold_resolved() {
        let r = review();
        let rows = plain(&r);
        use Row::*;
        // No header for a hunk at the top of the file, as in GitLab.
        assert_eq!(rows[..6], [File(0), Line(0, 0, 0), Thread(0, 0), Line(0, 0, 1), End(0), Gap]);
        // The resolved one opens on request.
        let open = super::rows(
            &r,
            &Shape { folded: &|_| false, open: &|id| id == "t2", above: &|_, _| 0, below: &|_| 0, lines: &|_| None, draft: None, parallel: false },
        );
        assert_eq!(open[3..5], [Line(0, 0, 1), Thread(0, 1)]);
        // A folded file is just its header.
        let folded = super::rows(
            &r,
            &Shape { folded: &|f| f.path() == "src/b.rs", open: &|_| false, above: &|_, _| 0, below: &|_| 0, lines: &|_| None, draft: None, parallel: false },
        );
        assert_eq!(folded[..2], [File(0), Gap]);
        // A file without hunks says why.
        assert!(rows.contains(&Message(2)));
    }

    #[test]
    fn expands_unchanged_lines_above_and_below() {
        let r = Review::build(
            MergeRequest::default(),
            vec![FileDiff {
                old_path: "m.rs".into(),
                new_path: "m.rs".into(),
                // Line 12 of the old file is line 10 of the new one.
                diff: "@@ -12,2 +10,2 @@\n x\n-a\n+b\n".into(),
                ..Default::default()
            }],
            vec![],
            Approvals::default(),
            vec![],
        );
        use Row::*;
        // Before the text is fetched: the header (9 lines hidden) and the end's button.
        let before = plain(&r);
        assert_eq!(before[..2], [File(0), Hunk(0, 0, 9)]);
        assert!(before.contains(&Expand(0)));
        // 5 shown above, with old numbers 2 ahead; 3 after the hunk of a 30-line file.
        let shape = Shape { folded: &|_| false, open: &|_| false, above: &|_, _| 5, below: &|_| 3, lines: &|_| Some(30), draft: None, parallel: false };
        let rows = super::rows(&r, &shape);
        assert_eq!(
            rows[..7],
            [File(0), Hunk(0, 0, 4), Context(0, 5, 7), Context(0, 6, 8), Context(0, 7, 9), Context(0, 8, 10), Context(0, 9, 11)]
        );
        let after: Vec<_> = rows.iter().skip_while(|r| !matches!(r, Line(0, 0, 2))).skip(1).copied().collect();
        assert_eq!(after, [Context(0, 12, 14), Context(0, 13, 15), Context(0, 14, 16), Expand(0), End(0), Gap]);
        // All of it: no header, no button.
        let all = Shape { folded: &|_| false, open: &|_| false, above: &|_, _| u32::MAX, below: &|_| u32::MAX, lines: &|_| Some(12), draft: None, parallel: false };
        let rows = super::rows(&r, &all);
        assert!(!rows.iter().any(|r| matches!(r, Hunk(..) | Expand(_))), "{rows:?}");
        assert_eq!(rows.iter().filter(|r| matches!(r, Context(..))).count(), 9 + 1);
        // Side by side: the changed line pairs up with its replacement.
        let side = Shape { parallel: true, ..all };
        let rows = super::rows(&r, &side);
        let pairs: Vec<_> = rows.iter().filter(|r| matches!(r, Pair(..))).copied().collect();
        assert_eq!(pairs, [Pair(0, 0, Some(0), Some(0)), Pair(0, 0, Some(1), Some(2))]);
    }

    #[test]
    fn searches_like_the_file_browser() {
        use super::matches;
        assert!(matches("src/limiter.rs", "limit"));
        assert!(matches("src/limiter.rs", "LIMIT"));
        assert!(matches("src/limiter.rs", "*.rs"));
        assert!(!matches("README.md", "*.rs"));
        assert!(matches("src/limiter.rs", "src/*"));
        assert!(!matches("docs/src.md", "src/*"));
        assert!(matches("src/a/b.rs", "src/*.rs"));
        assert!(matches("anything", " "));
    }

    #[test]
    fn tree_lists_folders_first() {
        let only: Vec<String> = tree(&review(), "*.md")
            .into_iter()
            .map(|t| match t {
                TreeItem::Folder { name, .. } | TreeItem::File { name, .. } => name,
            })
            .collect();
        assert_eq!(only, ["a", "README.md"]);
        let items: Vec<String> = tree(&review(), "")
            .into_iter()
            .map(|t| match t {
                TreeItem::Folder { name, depth, .. } => format!("{depth}d {name}"),
                TreeItem::File { name, depth, .. } => format!("{depth}f {name}"),
            })
            .collect();
        assert_eq!(items, ["0d a", "1f README.md", "0d src", "1f b.rs", "0f z.txt"]);
        // A folded folder keeps its row, not its files.
        let folded: Vec<String> = tree_folded(&review(), "", &|p| p == "src")
            .into_iter()
            .map(|t| match t {
                TreeItem::Folder { name, folded, .. } => format!("{name}{}", if folded { "+" } else { "" }),
                TreeItem::File { name, .. } => name,
            })
            .collect();
        assert_eq!(folded, ["a", "README.md", "src+", "z.txt"]);
    }
}
