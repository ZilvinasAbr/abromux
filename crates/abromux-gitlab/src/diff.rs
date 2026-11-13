//! Unified diff hunks, as GitLab sends them per file, parsed into lines with
//! their old and new line numbers.

use crate::{
    highlight::{Highlighter, Spans},
    types::FileDiff,
};
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Context,
    Added,
    Removed,
    /// `\ No newline at end of file`
    NoNewline,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub kind: Kind,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: String,
    /// Syntax tokens of `text` (`Body::of` fills them in).
    pub spans: Spans,
    /// The part that changed from its paired removed/added line, as GitLab
    /// marks it: between their common prefix and suffix.
    pub changed: Vec<Range<usize>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Hunk {
    /// The `@@ -a,b +c,d @@ context` line.
    pub header: String,
    pub old_start: u32,
    pub new_start: u32,
    pub lines: Vec<Line>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Body {
    /// Shared: a review is cloned to show a write before it lands.
    Text(std::sync::Arc<Vec<Hunk>>),
    Binary,
    /// A rename or mode change with no content change.
    Unchanged,
    /// GitLab didn't send it.
    TooLarge,
    /// GitLab collapsed it (big) and sent no diff with the list.
    Collapsed,
    /// Past GitLab's limits for one diff (1000 files, 200 KB a file): the
    /// review fetches both sides and diffs them itself, when it's shown.
    Pending,
}

impl Body {
    pub fn of(f: &FileDiff) -> Body {
        if f.too_large {
            return Body::TooLarge;
        }
        if f.diff.starts_with("Binary files ") {
            return Body::Binary;
        }
        let body = Body::text(&f.diff, &f.new_path);
        if body == Body::Unchanged && f.collapsed && f.diff.is_empty() {
            return Body::Collapsed;
        }
        body
    }

    /// A file GitLab didn't send, from both sides' bytes (`None`: no such
    /// file on that side): as GitLab would show it, 3 lines of context.
    pub fn local(old: Option<&[u8]>, new: Option<&[u8]>, path: &str) -> Body {
        let binary = |b: &[u8]| b[..b.len().min(8000)].contains(&0);
        if old.is_some_and(binary) || new.is_some_and(binary) {
            return Body::Binary;
        }
        let text = |b: Option<&[u8]>| {
            b.map(|b| String::from_utf8_lossy(b).into_owned())
                .unwrap_or_default()
        };
        let (old, new) = (text(old), text(new));
        // A pathological pair gives up on the best diff after a while, not on the diff.
        let diff = similar::TextDiff::configure()
            .timeout(std::time::Duration::from_secs(2))
            .diff_lines(&old, &new)
            .unified_diff()
            .context_radius(3)
            .to_string();
        Body::text(&diff, path)
    }

    /// A unified diff's hunks, highlighted, with GitLab's word changes.
    fn text(diff: &str, path: &str) -> Body {
        let mut hunks = parse(diff);
        if hunks.is_empty() {
            return Body::Unchanged;
        }
        for h in &mut hunks {
            for l in &mut h.lines {
                clip(&mut l.text);
            }
            let mut hl = Highlighter::new(path);
            for l in &mut h.lines {
                if l.kind != Kind::NoNewline {
                    l.spans = hl.line(&l.text);
                }
            }
            mark_changes(&mut h.lines);
        }
        Body::Text(std::sync::Arc::new(hunks))
    }

    pub fn hunks(&self) -> &[Hunk] {
        match self {
            Body::Text(h) => h,
            _ => &[],
        }
    }

    /// Lines added and removed.
    pub fn counts(&self) -> (usize, usize) {
        let lines = self.hunks().iter().flat_map(|h| &h.lines);
        lines.fold((0, 0), |(a, r), l| match l.kind {
            Kind::Added => (a + 1, r),
            Kind::Removed => (a, r + 1),
            _ => (a, r),
        })
    }
}

/// Past this many characters a line is cut short: laying out a minified
/// file's 300 KB line every frame would stall scrolling.
pub const LONG_LINE: usize = 5000;

fn clip(text: &mut String) {
    if text.len() > LONG_LINE {
        let cut = (0..=LONG_LINE)
            .rev()
            .find(|i| text.is_char_boundary(*i))
            .unwrap_or(0);
        let more = text[cut..].chars().count();
        text.truncate(cut);
        text.push_str(&format!(" … ({more} more characters)"));
    }
}

/// The hunks of a unified diff. Lines before the first `@@` (a `---`/`+++`
/// header, if any) are skipped, as is anything malformed.
pub fn parse(diff: &str) -> Vec<Hunk> {
    let mut hunks: Vec<Hunk> = vec![];
    let (mut old, mut new) = (0, 0);
    for raw in diff.lines() {
        if raw.starts_with("@@") {
            if let Some((o, n)) = hunk_starts(raw) {
                (old, new) = (o, n);
                hunks.push(Hunk {
                    header: raw.to_string(),
                    old_start: o,
                    new_start: n,
                    lines: vec![],
                });
            }
            continue;
        }
        let Some(h) = hunks.last_mut() else { continue };
        let (kind, text) = match raw.as_bytes().first() {
            Some(b'+') => (Kind::Added, &raw[1..]),
            Some(b'-') => (Kind::Removed, &raw[1..]),
            Some(b'\\') => (Kind::NoNewline, raw),
            Some(b' ') => (Kind::Context, &raw[1..]),
            // Some tools drop the space of an empty context line.
            None => (Kind::Context, ""),
            Some(_) => continue,
        };
        let (o, n) = match kind {
            Kind::Context => (Some(old), Some(new)),
            Kind::Added => (None, Some(new)),
            Kind::Removed => (Some(old), None),
            Kind::NoNewline => (None, None),
        };
        if o.is_some() {
            old += 1;
        }
        if n.is_some() {
            new += 1;
        }
        h.lines.push(Line {
            kind,
            old: o,
            new: n,
            text: text.to_string(),
            spans: vec![],
            changed: vec![],
        });
    }
    hunks
}

/// The rows of a side-by-side diff: (old side, new side) line indexes. A
/// context line is on both; a run of removed lines sits beside the run of
/// added lines after it, the shorter one padded with empty cells.
pub fn pairs(lines: &[Line]) -> Vec<(Option<usize>, Option<usize>)> {
    let mut out = vec![];
    let mut i = 0;
    while i < lines.len() {
        match lines[i].kind {
            Kind::Removed | Kind::Added => {
                let del = i;
                while i < lines.len() && lines[i].kind == Kind::Removed {
                    i += 1;
                }
                let add = i;
                while i < lines.len() && lines[i].kind == Kind::Added {
                    i += 1;
                }
                let (dels, adds) = (add - del, i - add);
                for k in 0..dels.max(adds) {
                    out.push(((k < dels).then_some(del + k), (k < adds).then_some(add + k)));
                }
            }
            // Context, and "\ No newline at end of file" on both sides.
            _ => {
                out.push((Some(i), Some(i)));
                i += 1;
            }
        }
    }
    out
}

/// Pair each run of removed lines with the run of added lines right after
/// it, line by line, and mark what changed in each pair.
pub fn mark_changes(lines: &mut [Line]) {
    let mut i = 0;
    while i < lines.len() {
        if lines[i].kind != Kind::Removed {
            i += 1;
            continue;
        }
        let del = i;
        while i < lines.len() && lines[i].kind == Kind::Removed {
            i += 1;
        }
        let add = i;
        while i < lines.len() && lines[i].kind == Kind::Added {
            i += 1;
        }
        for k in 0..(add - del).min(i - add) {
            let (old, new) = word_changes(&lines[del + k].text, &lines[add + k].text);
            lines[del + k].changed = old;
            lines[add + k].changed = new;
        }
    }
}

/// Byte ranges of a line's tokens: words, runs of spaces, and single
/// other characters.
fn tokens(s: &str) -> Vec<Range<usize>> {
    let mut out = vec![];
    let mut it = s.char_indices().peekable();
    while let Some((i, ch)) = it.next() {
        let word = |c: char| c.is_alphanumeric() || c == '_';
        let mut end = i + ch.len_utf8();
        if word(ch) || ch.is_whitespace() {
            let same = |c: char| if word(ch) { word(c) } else { c.is_whitespace() };
            while let Some(&(j, c)) = it.peek() {
                if !same(c) {
                    break;
                }
                end = j + c.len_utf8();
                it.next();
            }
        }
        out.push(i..end);
    }
    out
}

/// What changed between a removed line and the added line paired with it,
/// as GitLab marks it: the tokens not in their longest common subsequence,
/// merged where only a space separates them. Lines with nothing in common
/// (or very long ones) fall back to the differing middle.
pub fn word_changes(old: &str, new: &str) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let (a, b) = (tokens(old), tokens(new));
    let middle = || {
        let (o, n) = changed_ranges(old, new);
        (o.into_iter().collect(), n.into_iter().collect())
    };
    if a.len() * b.len() > 250_000 {
        return middle();
    }
    let (ta, tb): (Vec<&str>, Vec<&str>) = (
        a.iter().map(|r| &old[r.clone()]).collect(),
        b.iter().map(|r| &new[r.clone()]).collect(),
    );
    // LCS lengths from the end.
    let (n, m) = (ta.len(), tb.len());
    let mut l = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            l[i][j] = if ta[i] == tb[j] {
                l[i + 1][j + 1] + 1
            } else {
                l[i + 1][j].max(l[i][j + 1])
            };
        }
    }
    let (mut keep_a, mut keep_b) = (vec![false; n], vec![false; m]);
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if ta[i] == tb[j] {
            keep_a[i] = true;
            keep_b[j] = true;
            i += 1;
            j += 1;
        } else if l[i + 1][j] >= l[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    // Only spaces in common: say the line changed, not every word in it.
    let common =
        |keep: &[bool], t: &[&str]| keep.iter().zip(t).any(|(k, s)| *k && !s.trim().is_empty());
    if !common(&keep_a, &ta) {
        return middle();
    }
    let ranges = |keep: &[bool], toks: &[Range<usize>], t: &[&str]| {
        let mut out: Vec<Range<usize>> = vec![];
        let mut k = 0;
        while k < toks.len() {
            if keep[k] {
                k += 1;
                continue;
            }
            let start = toks[k].start;
            let mut end = toks[k].end;
            k += 1;
            loop {
                // A kept space between two changed tokens joins them.
                if k + 1 < toks.len() && keep[k] && t[k].trim().is_empty() && !keep[k + 1] {
                    end = toks[k + 1].end;
                    k += 2;
                } else if k < toks.len() && !keep[k] {
                    end = toks[k].end;
                    k += 1;
                } else {
                    break;
                }
            }
            out.push(start..end);
        }
        out
    };
    (ranges(&keep_a, &a, &ta), ranges(&keep_b, &b, &tb))
}

/// The differing middles of two lines, after their common prefix and
/// suffix (GitLab's inline diff). None for a side with nothing changed.
pub fn changed_ranges(old: &str, new: &str) -> (Option<Range<usize>>, Option<Range<usize>>) {
    let prefix: usize = old
        .chars()
        .zip(new.chars())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum();
    let max_suffix = old.len().min(new.len()) - prefix;
    let suffix: usize = old[prefix..]
        .chars()
        .rev()
        .zip(new[prefix..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum::<usize>()
        .min(max_suffix);
    let range = |s: &str| Some(prefix..s.len() - suffix).filter(|r| !r.is_empty());
    (range(old), range(new))
}

/// `@@ -3,5 +3,6 @@ …` → (3, 3). A start of 0 (an empty side) counts as 1:
/// it's where the first line would be.
fn hunk_starts(header: &str) -> Option<(u32, u32)> {
    let mut parts = header.split_whitespace().skip(1);
    let start = |p: &str, sign: char| -> Option<u32> {
        let n: u32 = p.strip_prefix(sign)?.split(',').next()?.parse().ok()?;
        Some(n.max(1))
    };
    let old = start(parts.next()?, '-')?;
    let new = start(parts.next()?, '+')?;
    Some((old, new))
}

#[cfg(test)]
// Lists of one changed range are what these tests expect.
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    #[test]
    fn local_diffs_read_like_gitlabs() {
        // Added: every line, from line 1.
        let Body::Text(h) = Body::local(None, Some(b"a\nb\n"), "x.txt") else {
            panic!()
        };
        assert_eq!((h.len(), h[0].new_start, h[0].lines.len()), (1, 1, 2));
        assert!(h[0].lines.iter().all(|l| l.kind == Kind::Added));
        // Modified: the change with 3 lines of context, as GitLab cuts it.
        let old: String = (1..=20).map(|n| format!("{n}\n")).collect();
        let new = old.replace("10\n", "ten\n");
        let Body::Text(h) = Body::local(Some(old.as_bytes()), Some(new.as_bytes()), "x.txt") else {
            panic!()
        };
        assert_eq!(h.len(), 1);
        assert_eq!(
            (h[0].old_start, h[0].new_start, h[0].lines.len()),
            (7, 7, 8)
        );
        // Deleted, binary, and the same on both sides.
        assert!(
            Body::local(Some(b"gone\n"), None, "x.txt").hunks()[0]
                .lines
                .iter()
                .all(|l| l.kind == Kind::Removed)
        );
        assert_eq!(
            Body::local(None, Some(b"\x89PNG\0\0"), "x.png"),
            Body::Binary
        );
        assert_eq!(
            Body::local(Some(b"same\n"), Some(b"same\n"), "x.txt"),
            Body::Unchanged
        );
        // A minified line is cut short, saying by how much.
        let long = format!("{}\n", "x".repeat(LONG_LINE + 1234));
        let Body::Text(h) = Body::local(None, Some(long.as_bytes()), "min.js") else {
            panic!()
        };
        assert!(h[0].lines[0].text.ends_with(" … (1234 more characters)"));
    }

    #[test]
    fn numbers_lines() {
        let h = parse("@@ -3,3 +3,3 @@ fn x\n a\n-b\n+B\n c\n\\ No newline at end of file\n");
        assert_eq!(h.len(), 1);
        let got: Vec<_> = h[0].lines.iter().map(|l| (l.kind, l.old, l.new)).collect();
        assert_eq!(
            got,
            [
                (Kind::Context, Some(3), Some(3)),
                (Kind::Removed, Some(4), None),
                (Kind::Added, None, Some(4)),
                (Kind::Context, Some(5), Some(5)),
                (Kind::NoNewline, None, None),
            ]
        );
        assert_eq!(h[0].lines[2].text, "B");
    }

    #[test]
    fn new_and_deleted_files_start_at_one() {
        let h = parse("@@ -0,0 +1,2 @@\n+a\n+b\n");
        assert_eq!(h[0].lines[1].new, Some(2));
        let h = parse("@@ -1,2 +0,0 @@\n-a\n-b\n");
        assert_eq!(h[0].lines[1].old, Some(2));
    }

    #[test]
    fn tells_why_there_is_no_diff() {
        let f = |collapsed, diff: &str, too_large| FileDiff {
            collapsed,
            too_large,
            diff: diff.into(),
            ..Default::default()
        };
        assert_eq!(Body::of(&f(true, "", false)), Body::Collapsed);
        assert_eq!(Body::of(&f(false, "", false)), Body::Unchanged);
        assert_eq!(Body::of(&f(false, "", true)), Body::TooLarge);
        assert_eq!(
            Body::of(&f(false, "Binary files a and b differ", false)),
            Body::Binary
        );
        assert!(matches!(
            Body::of(&f(true, "@@ -1 +1 @@\n-a\n+b\n", false)),
            Body::Text(_)
        ));
    }

    #[test]
    fn pairs_lines_side_by_side() {
        let h = parse("@@ -1,5 +1,4 @@\n a\n-b\n-c\n-d\n+B\n e\n+f\n");
        assert_eq!(
            pairs(&h[0].lines),
            [
                (Some(0), Some(0)),
                (Some(1), Some(4)),
                (Some(2), None),
                (Some(3), None),
                (Some(5), Some(5)),
                (None, Some(6)),
            ]
        );
    }

    #[test]
    fn marks_what_changed_like_gitlab() {
        let (old, new) = (
            "    Config { limit: 100 }",
            "    Config { limit: env(\"L\").or(100) }",
        );
        let (o, n) = changed_ranges(old, new);
        // The `)` breaks the common suffix, so `100` changed too.
        assert_eq!(&old[o.unwrap()], "100");
        assert_eq!(&new[n.unwrap()], "env(\"L\").or(100)");
        let (o, n) = changed_ranges("*n <= self.limit", "*n <= self.limit // inclusive");
        assert_eq!((o, n.map(|r| r.start)), (None, Some(16)));
        let (o, n) = changed_ranges("aaa", "aaa");
        assert_eq!((o, n), (None, None));
        // Overlapping prefix and suffix don't cross.
        let (o, n) = changed_ranges("aba", "abba");
        assert_eq!((o, n), (None, Some(2..3)));

        let mut h = parse("@@ -1,3 +1,3 @@\n-a1\n-b1\n+a2\n+b2\n c\n");
        mark_changes(&mut h[0].lines);
        // "a1" → "a2" have no token in common: the differing middle.
        let marks: Vec<_> = h[0].lines.iter().map(|l| l.changed.clone()).collect();
        assert_eq!(
            marks,
            [vec![1..2], vec![1..2], vec![1..2], vec![1..2], vec![]]
        );
    }

    #[test]
    fn marks_each_changed_word() {
        let old = "Find, devtools and console use its own shortcuts, set here";
        let new = "Find, devtools and console reach the tab too, set here";
        let (o, n) = word_changes(old, new);
        let pieces = |s: &str, r: &[Range<usize>]| {
            r.iter()
                .map(|r| s[r.clone()].to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(pieces(old, &o), ["use its own shortcuts"]);
        assert_eq!(pieces(new, &n), ["reach the tab too"]);
        // Two separate changes stay two.
        let (o, n) = word_changes("let a = f(1, x);", "let b = f(2, x);");
        assert_eq!(pieces("let a = f(1, x);", &o), ["a", "1"]);
        assert_eq!(pieces("let b = f(2, x);", &n), ["b", "2"]);
        // Nothing but spaces in common: the middle, as before.
        let (o, _) = word_changes("foo bar", "baz qux");
        assert_eq!(o, [0..7]);
    }

    #[test]
    fn several_hunks_and_junk() {
        let h = parse("--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n@@ -10,2 +10,2 @@\n x\n-y\n+z\n");
        assert_eq!(h.len(), 2);
        assert_eq!(h[1].lines[2].new, Some(11));
    }
}
