//! The command palette (⌘⇧P) and workspace switcher (⌘P), as in cmux.
//!
//! Commands are the app's actions with a title. Only the ones that apply to
//! what had focus when the palette opened are listed (terminal commands for
//! a terminal, browser ones for a browser), each with its shortcut. Choosing
//! one gives focus back and dispatches the action there, exactly as its
//! shortcut would.

use crate::{
    AbroMux, TITLEBAR_H, browser, channel, shortcut_digit, terminal, theme::*, workspace::Surface,
    workspace::tilde,
};
use gpui::{prelude::*, *};
use gpui_component::input::{Input, InputEvent, InputState};

actions!(palette, [SelectNext, SelectPrev, Confirm, Dismiss]);

/// Key context of the palette, for its bindings in main.rs.
pub const CONTEXT: &str = "CommandPalette";
const WIDTH: f32 = 560.;
const MAX_ROWS_HEIGHT: f32 = 360.;

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    Commands,
    Workspaces,
}

enum Target {
    Action(Box<dyn Action>),
    Workspace(usize),
}

struct Entry {
    title: SharedString,
    subtitle: Option<SharedString>,
    keys: Option<SharedString>,
    target: Target,
}

pub struct Palette {
    pub mode: Mode,
    input: Entity<InputState>,
    entries: Vec<Entry>,
    /// Indexes into `entries` that match the query, best first, with the
    /// matched character positions of each title.
    matches: Vec<(usize, Vec<usize>)>,
    selected: usize,
    /// Where focus goes back to, and where commands are dispatched.
    prev_focus: Option<FocusHandle>,
    scroll: ScrollHandle,
}

/// Commands in palette order. Ws1–9 and Tab1–9 are left out: the switcher
/// and the tab bar cover them.
fn commands() -> Vec<(&'static str, Box<dyn Action>)> {
    use crate::{
        CloseTab, CloseWorkspace, FlashPane, FocusAddressBar, FocusDown, FocusLeft, FocusRight,
        FocusUp, JumpToUnread, NewBrowser, NewTerminal, NewWorkspace, NextTab, NextWorkspace,
        PrevTab, PrevWorkspace, Quit, RenameTab, RenameWorkspace, ResizeDown, ResizeLeft,
        ResizeRight, ResizeUp, SplitDown, SplitRight, ToggleNotifications, ToggleSidebar,
        ToggleZoom,
    };
    vec![
        ("New Workspace", Box::new(NewWorkspace)),
        ("Rename Workspace…", Box::new(RenameWorkspace)),
        ("Close Workspace", Box::new(CloseWorkspace)),
        ("Next Workspace", Box::new(NextWorkspace)),
        ("Previous Workspace", Box::new(PrevWorkspace)),
        ("New Terminal Tab", Box::new(NewTerminal)),
        ("New Browser Tab", Box::new(NewBrowser)),
        ("Split Right", Box::new(SplitRight)),
        ("Split Down", Box::new(SplitDown)),
        ("Rename Tab…", Box::new(RenameTab)),
        ("Close Tab", Box::new(CloseTab)),
        ("Next Tab", Box::new(NextTab)),
        ("Previous Tab", Box::new(PrevTab)),
        ("Focus Pane Left", Box::new(FocusLeft)),
        ("Focus Pane Right", Box::new(FocusRight)),
        ("Focus Pane Up", Box::new(FocusUp)),
        ("Focus Pane Down", Box::new(FocusDown)),
        ("Resize Pane Left", Box::new(ResizeLeft)),
        ("Resize Pane Right", Box::new(ResizeRight)),
        ("Resize Pane Up", Box::new(ResizeUp)),
        ("Resize Pane Down", Box::new(ResizeDown)),
        ("Flash Focused Pane", Box::new(FlashPane)),
        ("Toggle Pane Zoom", Box::new(ToggleZoom)),
        ("Toggle Sidebar", Box::new(ToggleSidebar)),
        ("Show Notifications", Box::new(ToggleNotifications)),
        ("Jump to Latest Unread", Box::new(JumpToUnread)),
        ("Copy", Box::new(terminal::Copy)),
        ("Paste", Box::new(terminal::Paste)),
        ("Find…", Box::new(terminal::Find)),
        ("Find Next", Box::new(terminal::FindNext)),
        ("Find Previous", Box::new(terminal::FindPrevious)),
        (
            "Use Selection for Find",
            Box::new(terminal::UseSelectionForFind),
        ),
        ("Clear Scrollback", Box::new(terminal::ClearScrollback)),
        ("Restart Shell", Box::new(terminal::RestartShell)),
        ("Increase Font Size", Box::new(terminal::IncreaseFontSize)),
        ("Decrease Font Size", Box::new(terminal::DecreaseFontSize)),
        ("Reset Font Size", Box::new(terminal::ResetFontSize)),
        ("Focus Address Bar", Box::new(FocusAddressBar)),
        ("Back", Box::new(browser::Back)),
        ("Forward", Box::new(browser::Forward)),
        ("Reload Page", Box::new(browser::Reload)),
        ("Toggle Developer Tools", Box::new(browser::ToggleDevTools)),
        ("Show JavaScript Console", Box::new(browser::ShowConsole)),
        ("Quit", Box::new(Quit)),
    ]
}

impl AbroMux {
    /// ⌘⇧P / ⌘P: open the palette in `mode`, switch an open one to it, or
    /// close it if it's already showing `mode`.
    pub(crate) fn toggle_palette(
        &mut self,
        mode: Mode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let prev_focus = match self.palette.take() {
            Some(p) if p.mode == mode => {
                self.restore_focus(p.prev_focus, window, cx);
                return cx.notify();
            }
            // Switching modes: focus goes back first, so commands are
            // worked out for the pane rather than the palette's input.
            Some(p) => {
                if let Some(f) = &p.prev_focus {
                    window.focus(f);
                }
                p.prev_focus
            }
            None => window.focused(cx),
        };
        self.notifications_open = false;
        let entries = match mode {
            Mode::Commands => self.command_entries(window, cx),
            Mode::Workspaces => self.workspace_entries(cx),
        };
        let placeholder = match mode {
            Mode::Commands => "Type a command",
            Mode::Workspaces => "Search workspaces",
        };
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        cx.subscribe_in(
            &input,
            window,
            |this, input, ev: &InputEvent, _, cx| match ev {
                InputEvent::Change => {
                    let query = input.read(cx).value().to_string();
                    if let Some(p) = &mut this.palette {
                        p.filter(&query);
                        cx.notify();
                    }
                }
                // Focus went somewhere else (a click outside, a pane taking it).
                InputEvent::Blur if this.palette.as_ref().is_some_and(|p| &p.input == input) => {
                    this.palette = None;
                    cx.notify();
                }
                _ => {}
            },
        )
        .detach();
        input.update(cx, |i, cx| i.focus(window, cx));
        let mut palette = Palette {
            mode,
            input,
            entries,
            matches: vec![],
            selected: 0,
            prev_focus,
            scroll: ScrollHandle::new(),
        };
        palette.filter("");
        self.palette = Some(palette);
        cx.notify();
    }

    pub(crate) fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = self.palette.take() {
            self.restore_focus(p.prev_focus, window, cx);
            cx.notify();
        }
    }

    fn restore_focus(
        &mut self,
        prev: Option<FocusHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match prev {
            Some(f) => window.focus(&f),
            None => self.focus_active(window, cx),
        }
    }

    fn confirm_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut p) = self.palette.take() else {
            return;
        };
        let Some(&(ix, _)) = p.matches.get(p.selected) else {
            self.palette = Some(p);
            return;
        };
        let entry = p.entries.swap_remove(ix);
        cx.notify();
        match entry.target {
            Target::Action(action) => {
                // Dispatched where focus was, so terminal and browser
                // commands reach their view.
                self.restore_focus(p.prev_focus, window, cx);
                window.dispatch_action(action, cx);
            }
            Target::Workspace(id) => match self.workspaces.iter().position(|w| w.id == id) {
                Some(i) => self.select_workspace(i, window, cx),
                None => self.restore_focus(p.prev_focus, window, cx),
            },
        }
    }

    /// The commands that apply to what has focus now, with their shortcuts.
    fn command_entries(&self, window: &Window, cx: &mut App) -> Vec<Entry> {
        let browser = matches!(self.ws().focused_surface(), Some(Surface::Browser(_)));
        let available: Vec<_> = commands()
            .into_iter()
            // Root actions are always available; this drops terminal ones
            // when a browser has focus and the other way round.
            .filter(|(_, a)| window.is_action_available(a.as_ref(), cx))
            // Its handler sits on the root, but it only acts on a browser.
            .filter(|(_, a)| browser || !a.as_any().is::<crate::FocusAddressBar>())
            .collect();
        let stack = window.context_stack();
        let keymap = cx.key_bindings();
        let keymap = keymap.borrow();
        available
            .into_iter()
            .map(|(title, action)| {
                let title = match title {
                    "Quit" => format!("Quit {}", channel::app_name()),
                    t => t.to_string(),
                };
                // The first binding whose context applies is the main one
                // (⌘⇧] before ⌃⇥). gpui's own lookup would hide ⌥⌘I behind
                // gpui-component's context-free inspector binding, which
                // never runs.
                let keys = keymap
                    .bindings_for_action(action.as_ref())
                    .find(|b| b.predicate().is_none_or(|p| p.depth_of(&stack).is_some()))
                    .map(|b| {
                        b.keystrokes()
                            .iter()
                            .map(|k| key_label(k.modifiers(), k.key()))
                            .collect::<Vec<_>>()
                            .join(" ")
                            .into()
                    });
                Entry {
                    title: title.into(),
                    subtitle: None,
                    keys,
                    target: Target::Action(action),
                }
            })
            .collect()
    }

    fn workspace_entries(&self, cx: &App) -> Vec<Entry> {
        let n = self.workspaces.len();
        self.workspaces
            .iter()
            .enumerate()
            .map(|(i, ws)| Entry {
                title: ws.name(cx).into(),
                subtitle: ws.cwd(cx).map(|c| tilde(&c).into()),
                keys: shortcut_digit(i, n).map(|d| format!("⌘{d}").into()),
                target: Target::Workspace(ws.id),
            })
            .collect()
    }

    /// Handlers for the palette's keys, put on the root element.
    pub(crate) fn palette_actions(
        &self,
        root: Stateful<Div>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        root.on_action(cx.listener(|this, _: &SelectNext, _, cx| this.move_selection(1, cx)))
            .on_action(cx.listener(|this, _: &SelectPrev, _, cx| this.move_selection(-1, cx)))
            .on_action(
                cx.listener(|this, _: &Confirm, window, cx| this.confirm_palette(window, cx)),
            )
            .on_action(cx.listener(|this, _: &Dismiss, window, cx| this.close_palette(window, cx)))
    }

    fn move_selection(&mut self, by: isize, cx: &mut Context<Self>) {
        let Some(p) = &mut self.palette else { return };
        let n = p.matches.len() as isize;
        if n == 0 {
            return;
        }
        p.selected = (p.selected as isize + by).rem_euclid(n) as usize;
        p.scroll.scroll_to_item(p.selected);
        cx.notify();
    }

    pub(crate) fn render_palette(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = self.palette.as_ref()?;
        let rows: Vec<AnyElement> = p
            .matches
            .iter()
            .enumerate()
            .map(|(row, (ix, positions))| {
                let e = &p.entries[*ix];
                let selected = row == p.selected;
                div()
                    .id(("palette-row", row))
                    .mx(px(6.))
                    .px(px(10.))
                    .py(px(6.))
                    .rounded(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .cursor_pointer()
                    .when(selected, |d| d.bg(rgba(0xffffff1a)))
                    .when(!selected, |d| d.hover(|s| s.bg(rgba(HOVER))))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(p) = &mut this.palette {
                            p.selected = row;
                        }
                        this.confirm_palette(window, cx);
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_baseline()
                            .gap(px(8.))
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(13.))
                                    .child(highlighted(&e.title, positions)),
                            )
                            .children(e.subtitle.clone().map(|s| {
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(11.5))
                                    .text_color(rgb(MUTED))
                                    .child(s)
                            })),
                    )
                    .children(e.keys.clone().map(|k| {
                        div()
                            .flex_none()
                            .text_size(px(11.5))
                            .text_color(rgb(MUTED))
                            .child(k)
                    }))
                    .into_any_element()
            })
            .collect();
        let empty = match p.mode {
            Mode::Commands => "No commands match your search.",
            Mode::Workspaces => "No workspaces match your search.",
        };
        Some(
            div()
                .absolute()
                .inset_0()
                // Clicking outside closes it.
                .child(
                    div()
                        .id("palette-backdrop")
                        .absolute()
                        .inset_0()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| this.close_palette(window, cx)),
                        ),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(TITLEBAR_H + 8.))
                        .left_0()
                        .right_0()
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .id("palette")
                                .key_context(CONTEXT)
                                .w(px(WIDTH))
                                .flex()
                                .flex_col()
                                .rounded(px(10.))
                                .bg(rgb(TABBAR_BG))
                                .border_1()
                                .border_color(rgb(BORDER))
                                .shadow_lg()
                                .overflow_hidden()
                                .occlude()
                                // Keep focus in the input when a row is clicked.
                                .on_mouse_down(MouseButton::Left, |_, window, _| {
                                    window.prevent_default()
                                })
                                .child(
                                    div()
                                        .h(px(40.))
                                        .px(px(14.))
                                        .flex()
                                        .items_center()
                                        .border_b_1()
                                        .border_color(rgb(BORDER))
                                        .child(
                                            Input::new(&p.input)
                                                .appearance(false)
                                                .text_size(px(14.))
                                                .h_full()
                                                .px_0()
                                                .py_0(),
                                        ),
                                )
                                .child(
                                    div()
                                        .id("palette-rows")
                                        .max_h(px(MAX_ROWS_HEIGHT))
                                        .py(px(6.))
                                        .overflow_y_scroll()
                                        .track_scroll(&p.scroll)
                                        .when(rows.is_empty(), |d| {
                                            d.child(
                                                div()
                                                    .px(px(16.))
                                                    .py(px(8.))
                                                    .text_size(px(12.5))
                                                    .text_color(rgb(MUTED))
                                                    .child(empty),
                                            )
                                        })
                                        .children(rows),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}

impl Palette {
    fn filter(&mut self, query: &str) {
        let mut scored: Vec<(i32, usize, Vec<usize>)> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| fuzzy_match(query, &e.title).map(|(score, pos)| (score, i, pos)))
            .collect();
        // Best score first; ties keep palette order.
        scored.sort_by_key(|(score, i, _)| (-score, *i));
        self.matches = scored.into_iter().map(|(_, i, pos)| (i, pos)).collect();
        self.selected = 0;
        self.scroll.scroll_to_item(0);
    }
}

/// The title with matched characters in the accent colour.
fn highlighted(title: &SharedString, positions: &[usize]) -> StyledText {
    let style = HighlightStyle {
        color: Some(rgb(ACCENT_BRIGHT).into()),
        font_weight: Some(FontWeight::SEMIBOLD),
        ..Default::default()
    };
    let highlights = title
        .char_indices()
        .enumerate()
        .filter(|(ci, _)| positions.contains(ci))
        .map(|(_, (b, c))| (b..b + c.len_utf8(), style))
        .collect::<Vec<_>>();
    StyledText::new(title.clone()).with_highlights(highlights)
}

/// A shortcut the way macOS menus show it: ⌃⌥⇧⌘ then the key.
fn key_label(m: &Modifiers, key: &str) -> String {
    // gpui folds shift into these keys (cmd-} is ⌘⇧]).
    let (shift, key) = match key {
        "{" => (true, "["),
        "}" => (true, "]"),
        k => (m.shift, k),
    };
    let mut s = String::new();
    for (on, sym) in [
        (m.control, '⌃'),
        (m.alt, '⌥'),
        (shift, '⇧'),
        (m.platform, '⌘'),
    ] {
        if on {
            s.push(sym);
        }
    }
    s.push_str(&match key {
        "left" => "←".into(),
        "right" => "→".into(),
        "up" => "↑".into(),
        "down" => "↓".into(),
        "tab" => "⇥".into(),
        "enter" => "↩".into(),
        "escape" => "⎋".into(),
        "backspace" => "⌫".into(),
        "space" => "Space".into(),
        k => k.to_uppercase(),
    });
    s
}

/// Case-insensitive fuzzy match of `query` in `text`. Returns a score
/// (higher is better) and the char positions matched, or None. A substring
/// beats scattered letters; matches at word starts and the start of the
/// text score higher.
pub fn fuzzy_match(query: &str, text: &str) -> Option<(i32, Vec<usize>)> {
    let q: Vec<char> = query.trim().chars().flat_map(char::to_lowercase).collect();
    if q.is_empty() {
        return Some((0, vec![]));
    }
    let t: Vec<char> = text
        .chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect();
    let word_start = |i: usize| i == 0 || !t[i - 1].is_alphanumeric();

    // A contiguous run, preferring one that starts a word.
    let runs = (0..t.len()).filter(|&i| t[i..].starts_with(&q));
    if let Some(start) = runs
        .clone()
        .find(|&i| word_start(i))
        .or_else(|| runs.clone().next())
    {
        let score =
            1000 + if start == 0 {
                200
            } else if word_start(start) {
                100
            } else {
                0
            } - start as i32;
        return Some((score, (start..start + q.len()).collect()));
    }

    // Letters in order, each taken at a word start if one is ahead before
    // the next letter is needed, else the first occurrence.
    let mut pos = Vec::with_capacity(q.len());
    let mut from = 0;
    for (qi, &c) in q.iter().enumerate() {
        let rest = &q[qi + 1..];
        let candidates: Vec<usize> = (from..t.len()).filter(|&i| t[i] == c).collect();
        let fits = |i: usize| is_subsequence(rest, &t[i + 1..]);
        let pick = candidates
            .iter()
            .copied()
            .find(|&i| word_start(i) && fits(i))
            .or_else(|| candidates.iter().copied().find(|&i| fits(i)))?;
        pos.push(pick);
        from = pick + 1;
    }
    let starts = pos.iter().filter(|&&i| word_start(i)).count() as i32;
    let gaps: i32 = pos.windows(2).map(|w| (w[1] - w[0] - 1) as i32).sum();
    Some((500 + 20 * starts - gaps - pos[0] as i32, pos))
}

fn is_subsequence(needle: &[char], hay: &[char]) -> bool {
    let mut it = hay.iter();
    needle.iter().all(|c| it.any(|h| h == c))
}

#[cfg(test)]
mod tests {
    use super::{fuzzy_match, key_label};
    use gpui::Modifiers;

    fn score(q: &str, t: &str) -> i32 {
        fuzzy_match(q, t).map(|(s, _)| s).unwrap_or(i32::MIN)
    }

    #[test]
    fn matches_and_positions() {
        assert_eq!(fuzzy_match("", "Split Right"), Some((0, vec![])));
        assert_eq!(
            fuzzy_match("right", "Split Right").unwrap().1,
            vec![6, 7, 8, 9, 10]
        );
        assert_eq!(fuzzy_match("SR", "Split Right").unwrap().1, vec![0, 6]);
        assert_eq!(
            fuzzy_match("ntt", "New Terminal Tab").unwrap().1,
            vec![0, 4, 13]
        );
        assert!(fuzzy_match("xyz", "Split Right").is_none());
        assert!(fuzzy_match("thgir", "Split Right").is_none());
    }

    #[test]
    fn ranking() {
        // A substring beats letters spread out.
        assert!(score("tab", "Next Tab") > score("tab", "Toggle Address Bar"));
        // A word start beats the middle of a word.
        assert!(score("in", "Increase Font Size") > score("in", "Find Next"));
        // Earlier is better.
        assert!(score("split", "Split Down") > score("split", "Something Split"));
        // Word starts beat scattered letters.
        assert!(score("cw", "Close Workspace") > score("cw", "Clear Scrollback Window"));
    }

    #[test]
    fn non_ascii() {
        assert_eq!(fuzzy_match("ž", "Žilvinas").unwrap().1, vec![0]);
        assert!(fuzzy_match("rename", "Rename Workspace…").is_some());
    }

    #[test]
    fn key_labels() {
        let cmd = Modifiers {
            platform: true,
            ..Default::default()
        };
        let cmd_shift = Modifiers {
            platform: true,
            shift: true,
            ..Default::default()
        };
        let ctrl_shift = Modifiers {
            control: true,
            shift: true,
            ..Default::default()
        };
        assert_eq!(key_label(&cmd, "}"), "⇧⌘]");
        assert_eq!(key_label(&cmd_shift, "p"), "⇧⌘P");
        assert_eq!(key_label(&ctrl_shift, "h"), "⌃⇧H");
        assert_eq!(
            key_label(
                &Modifiers {
                    alt: true,
                    platform: true,
                    ..Default::default()
                },
                "left"
            ),
            "⌥⌘←"
        );
    }
}
