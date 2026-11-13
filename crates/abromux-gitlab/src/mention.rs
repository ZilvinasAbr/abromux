//! `@name` in a comment, as GitLab's text boxes complete it: typing `@`
//! and some of a name or username lists the people it could be (who's on
//! the merge request first, then the project's members; on a big project
//! GitLab is asked as you type). ↑ ↓ pick, Enter or Tab puts the username
//! in, Esc closes the list.

#[allow(unused_imports)]
use crate::style::px;
use crate::{style::*, types::User, view::GitLabView};
use gpui::{prelude::*, *};
use gpui_component::input::{self, InputState};
use std::ops::Range;

/// The text boxes that complete names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Comment,
    Reply,
    Line,
}

/// The list under a box: the `@query` being typed and the row picked.
#[derive(Clone, Debug, PartialEq)]
pub struct Mention {
    pub field: Field,
    /// Where `@query` is in the box's text, in bytes.
    pub range: Range<usize>,
    pub query: String,
    pub selected: usize,
}

/// How many people the list shows.
const ROWS: usize = 8;

/// The `@query` that ends at `cursor` (a byte offset into `text`): an `@`
/// at the start of a word, then letters, digits, `_`, `-` or `.` up to the
/// cursor. `@` alone lists everyone.
pub fn at_cursor(text: &str, cursor: usize) -> Option<(Range<usize>, String)> {
    let before = text.get(..cursor)?;
    let at = before.rfind('@')?;
    let query = &before[at + 1..];
    let name = |c: char| c.is_alphanumeric() || matches!(c, '_' | '-' | '.');
    if query.chars().count() > 40 || !query.chars().all(name) {
        return None;
    }
    // `x@y` is an address, not a mention.
    if before[..at]
        .chars()
        .next_back()
        .is_some_and(|p| !(p.is_whitespace() || matches!(p, '(' | '[' | '>' | ',' | ';' | ':')))
    {
        return None;
    }
    Some((at..cursor, query.to_string()))
}

/// How well `u` matches `query` (lowercase): 0 a username or a name's word
/// starts with it, 1 it's somewhere in either; None: not at all.
fn rank(u: &User, query: &str) -> Option<u8> {
    let (name, username) = (u.name.to_lowercase(), u.username.to_lowercase());
    if query.is_empty()
        || username.starts_with(query)
        || name.split_whitespace().any(|w| w.starts_with(query))
    {
        Some(0)
    } else if username.contains(query) || name.contains(query) {
        Some(1)
    } else {
        None
    }
}

/// The people `query` could mean out of `pools` (the earlier pool first),
/// best matches first, each once, at most `ROWS`.
pub fn matches<'a>(pools: &[&'a [User]], query: &str) -> Vec<&'a User> {
    let query = query.to_lowercase();
    let mut found: Vec<(u8, usize, &User)> = vec![];
    for (p, pool) in pools.iter().enumerate() {
        for u in pool.iter().filter(|u| u.id != 0 && !u.username.is_empty()) {
            if found.iter().any(|(_, _, f)| f.id == u.id) {
                continue;
            }
            if let Some(r) = rank(u, &query) {
                found.push((r, p, u));
            }
        }
    }
    // Stable: within a rank and a pool, the pool's own order.
    found.sort_by_key(|(r, p, _)| (*r, *p));
    found.into_iter().take(ROWS).map(|(_, _, u)| u).collect()
}

impl GitLabView {
    fn mention_box(&self, field: Field) -> &Entity<InputState> {
        match field {
            Field::Comment => &self.boxes.comment,
            Field::Reply => &self.boxes.reply,
            Field::Line => &self.boxes.line,
        }
    }

    /// The people the open list shows.
    pub(crate) fn mention_people(&self) -> Vec<User> {
        let (Some(m), Some(r)) = (&self.mention, self.review.ready()) else {
            return vec![];
        };
        // Who's on the merge request, as the sidebar's participants.
        let mut on_it: Vec<User> = vec![r.mr.author.clone()];
        let threads = r.general.iter().chain(&r.outdated).chain(
            r.files
                .iter()
                .flat_map(|f| f.threads.iter().map(|(_, _, d)| d)),
        );
        let noted = threads.flat_map(|d| d.notes.iter().filter(|n| !n.system).map(|n| &n.author));
        for u in r.mr.assignees.iter().chain(&r.mr.reviewers).chain(noted) {
            if !on_it.iter().any(|p| p.id == u.id) {
                on_it.push(u.clone());
            }
        }
        // Not yourself: GitLab leaves you out too.
        let me = self.me.as_ref().map_or(0, |u| u.id);
        on_it.retain(|u| u.id != me);
        let found: &[User] = match &self.mention_found {
            Some((q, people)) if *q == m.query.to_lowercase() => people,
            _ => &[],
        };
        let members: Vec<User> = r.members.iter().filter(|u| u.id != me).cloned().collect();
        matches(&[&on_it, found, &members], &m.query)
            .into_iter()
            .cloned()
            .collect()
    }

    /// The box `field` changed: open, follow or close its list.
    pub(crate) fn update_mention(&mut self, field: Field, cx: &mut Context<Self>) {
        let b = self.mention_box(field).read(cx);
        let at = if self.is_local() {
            None
        } else {
            at_cursor(&b.value(), b.cursor())
        };
        let Some((range, query)) = at else {
            if self.mention.as_ref().is_some_and(|m| m.field == field) {
                self.mention = None;
            }
            return;
        };
        let same = self
            .mention
            .as_ref()
            .is_some_and(|m| m.field == field && m.query == query);
        let selected = if same {
            self.mention.as_ref().map_or(0, |m| m.selected)
        } else {
            0
        };
        self.mention = Some(Mention {
            field,
            range,
            query: query.clone(),
            selected,
        });
        if !same {
            self.search_mention(query, cx);
        }
    }

    /// On a project with more members than were loaded, ask GitLab who
    /// `query` could be; the latest query's answer is kept.
    fn search_mention(&mut self, query: String, cx: &mut Context<Self>) {
        let (Some(r), Some(c)) = (self.review.ready().cloned(), self.client.clone()) else {
            return;
        };
        let query = query.to_lowercase();
        if !r.members_more || query.is_empty() {
            return;
        }
        let project = r.mr.project_id;
        self._mention_task = Some(cx.spawn(async move |this, cx| {
            let q = query.clone();
            let got = cx
                .background_spawn(async move { c.search_members(project, &q) })
                .await;
            this.update(cx, |v, cx| {
                if let Ok(found) = got {
                    let mut people: Vec<User> = vec![];
                    for m in found
                        .iter()
                        .filter(|m| m.state.is_empty() || m.state == "active")
                    {
                        if !people.iter().any(|p| p.id == m.id) {
                            people.push(m.user());
                        }
                    }
                    v.avatars
                        .want(people.iter().collect::<Vec<_>>(), cx, |v| &mut v.avatars);
                    v.mention_found = Some((query, people));
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    fn move_mention(&mut self, by: isize, cx: &mut Context<Self>) {
        let n = self.mention_people().len();
        if let Some(m) = &mut self.mention
            && n > 0
        {
            m.selected = (m.selected.min(n - 1) as isize + by).rem_euclid(n as isize) as usize;
            cx.notify();
        }
    }

    /// Put the picked person's `@username` where `@query` is. False: there
    /// was no one to put.
    pub(crate) fn accept_mention(
        &mut self,
        index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let people = self.mention_people();
        let Some(m) = self.mention.take() else {
            return false;
        };
        let Some(u) = people.get(
            index
                .unwrap_or(m.selected)
                .min(people.len().saturating_sub(1)),
        ) else {
            return false;
        };
        let insert = format!("@{} ", u.username);
        self.mention_box(m.field).clone().update(cx, |b, cx| {
            let text = b.value();
            // Still as it was when the list opened.
            if text
                .get(m.range.clone())
                .is_none_or(|t| t.strip_prefix('@') != Some(m.query.as_str()))
            {
                return;
            }
            let utf16 = |i: usize| text[..i].encode_utf16().count();
            let range = utf16(m.range.start)..utf16(m.range.end);
            EntityInputHandler::replace_text_in_range(b, Some(range), &insert, window, cx);
        });
        cx.notify();
        true
    }

    /// A text box that completes `@names`: `input` with the keys the open
    /// list takes (before the box does), and the list under it.
    pub(crate) fn mention_input(
        &self,
        field: Field,
        input: impl IntoElement,
        cx: &mut Context<Self>,
    ) -> Div {
        let open = self.mention.as_ref().filter(|m| m.field == field);
        let people = if open.is_some() {
            self.mention_people()
        } else {
            vec![]
        };
        let listing = !people.is_empty();
        let mut wrap = div().flex().flex_col().gap_1().child(input);
        if listing {
            wrap = wrap
                .capture_action(cx.listener(|v, _: &input::MoveUp, _, cx| {
                    v.move_mention(-1, cx);
                    cx.stop_propagation();
                }))
                .capture_action(cx.listener(|v, _: &input::MoveDown, _, cx| {
                    v.move_mention(1, cx);
                    cx.stop_propagation();
                }))
                .capture_action(cx.listener(|v, _: &input::Enter, window, cx| {
                    if v.accept_mention(None, window, cx) {
                        cx.stop_propagation();
                    }
                }))
                .capture_action(cx.listener(|v, _: &input::IndentInline, window, cx| {
                    if v.accept_mention(None, window, cx) {
                        cx.stop_propagation();
                    }
                }))
                .capture_action(cx.listener(|v, _: &input::Escape, _, cx| {
                    v.mention = None;
                    cx.stop_propagation();
                    cx.notify();
                }));
        }
        let selected = open
            .map_or(0, |m| m.selected)
            .min(people.len().saturating_sub(1));
        wrap.when(listing, |d| {
            d.child(
                div()
                    .id("gl-mentions")
                    .flex()
                    .flex_col()
                    .py_1()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(c(BORDER))
                    .bg(c(RAISED))
                    // A click here mustn't take the keys from the box.
                    .on_mouse_down(MouseButton::Left, |_, window, cx| {
                        window.prevent_default();
                        cx.stop_propagation();
                    })
                    .children(people.iter().enumerate().map(|(i, u)| {
                        div()
                            .id(("gl-mention", u.id as usize))
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_2()
                            .h(px(32.))
                            .cursor_pointer()
                            .when(i == selected, |d| d.bg(ca(0x428fdc55)))
                            .hover(|s| s.bg(ca(0x89888d55)))
                            .child(self.avatars.of(u, 24.))
                            .child(
                                div()
                                    .flex_none()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(c(TEXT))
                                    .child(u.name.clone()),
                            )
                            .child(
                                div()
                                    .min_w(px(0.))
                                    .truncate()
                                    .text_color(c(MUTED))
                                    .child(format!("@{}", u.username)),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |v, _, window, cx| {
                                    window.prevent_default();
                                    cx.stop_propagation();
                                    v.accept_mention(Some(i), window, cx);
                                }),
                            )
                    })),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{at_cursor, matches};
    use crate::types::User;

    #[test]
    fn finds_the_mention_being_typed() {
        let at = |text: &str| at_cursor(text, text.len()).map(|(r, q)| (r.start, q));
        assert_eq!(at("@"), Some((0, "".into())));
        assert_eq!(at("thanks @bo"), Some((7, "bo".into())));
        assert_eq!(at("(@žil"), Some((1, "žil".into())));
        assert_eq!(at("line one\n@a.b-c_d"), Some((9, "a.b-c_d".into())));
        // Not one: an address, a space after the name, no @ at all.
        assert_eq!(at("mail x@y"), None);
        assert_eq!(at("@bob "), None);
        assert_eq!(at("hello"), None);
        // The cursor in the middle: what's before it.
        assert_eq!(at_cursor("@bobby and", 4), Some((0..4, "bob".into())));
        assert_eq!(at_cursor("žž", 1), None, "not a char boundary");
    }

    #[test]
    fn matches_names_and_usernames() {
        let u = |id, name: &str, username: &str| User {
            id,
            name: name.into(),
            username: username.into(),
            ..Default::default()
        };
        let on_it = [u(1, "Bob Stone", "bstone"), u(2, "Alice Roberts", "alice")];
        let members = [
            u(3, "Robert Boyle", "rb"),
            u(1, "Bob Stone", "bstone"),
            u(4, "Jacob", "jake"),
            u(0, "Ghost", ""),
        ];
        let names = |q: &str| {
            matches(&[&on_it, &members], q)
                .iter()
                .map(|u| u.username.as_str())
                .collect::<Vec<_>>()
        };
        // By first name, last name or username; who's on it first; once each.
        assert_eq!(names("bo"), ["bstone", "rb"]);
        assert_eq!(names("ob"), ["bstone", "alice", "rb", "jake"]);
        assert_eq!(names("ROB"), ["alice", "rb"]);
        assert_eq!(names("jak"), ["jake"]);
        assert_eq!(names(""), ["bstone", "alice", "rb", "jake"]);
        assert_eq!(names("zzz"), Vec::<&str>::new());
    }
}
