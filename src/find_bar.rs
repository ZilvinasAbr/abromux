//! The ⌘F bar terminals and browser panes share: its state, its row of
//! controls, and which way a step goes. Each pane places the bar (over the
//! terminal, above the page) and runs its own search.

use crate::theme::*;
use gpui::{prelude::*, *};
use gpui_component::input::{self, Input, InputState};

/// Which way a step goes on screen. The ↑ button is always [`Up`](Step::Up);
/// what Return and ⌘G mean is each pane's call (a terminal goes up to older
/// output, a page down to the next match).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Up,
    Down,
}

#[derive(Default)]
pub struct FindBar {
    /// Kept after closing, so ⌘F brings the last search back.
    pub input: Option<Entity<InputState>>,
    pub open: bool,
    /// Match count and selected index, for the counter.
    pub count: (usize, Option<usize>),
}

impl FindBar {
    /// The input, while the bar is open.
    pub fn shown(&self) -> Option<Entity<InputState>> {
        self.input.clone().filter(|_| self.open)
    }

    /// Closed, with nothing counted.
    pub fn close(&mut self) {
        self.open = false;
        self.count = (0, None);
    }

    /// `3/12`, or `-/12` with no match selected.
    fn counter(&self) -> String {
        match self.count {
            (total, Some(i)) => format!("{}/{total}", i + 1),
            (total, None) => format!("-/{total}"),
        }
    }

    /// The bar's row: icon, field, counter and the ↑ ↓ × buttons, wired to
    /// the pane's `step` and `close`. The pane adds where it sits.
    pub fn render<V: 'static>(
        &self,
        input: &Entity<InputState>,
        step: fn(&mut V, Step, &mut Context<V>),
        close: fn(&mut V, &mut Window, &mut Context<V>),
        cx: &mut Context<V>,
    ) -> Stateful<Div> {
        div()
            .id("find-bar")
            .key_context("FindBar")
            .h(px(32.))
            .px_2()
            .flex()
            .items_center()
            .gap_1()
            .bg(rgb(TABBAR_BG))
            .text_size(px(12.))
            .text_color(rgb(TEXT))
            .on_action(
                cx.listener(move |this, _: &input::Escape, window, cx| close(this, window, cx)),
            )
            .child(icon("search"))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Input::new(input).appearance(false).text_size(px(12.))),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(rgb(MUTED))
                    .child(self.counter()),
            )
            .child(
                icon_button("find-up", "chevron-up")
                    .on_click(cx.listener(move |this, _, _, cx| step(this, Step::Up, cx))),
            )
            .child(
                icon_button("find-down", "chevron-down")
                    .on_click(cx.listener(move |this, _, _, cx| step(this, Step::Down, cx))),
            )
            .child(
                icon_button("find-close", "x")
                    .on_click(cx.listener(move |this, _, window, cx| close(this, window, cx))),
            )
    }
}
