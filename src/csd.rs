//! Client-side decorations. Most Wayland compositors decorate windows
//! themselves, or don't (Hyprland, sway, KDE), and the window asks them to
//! (`WindowDecorations::Server`). GNOME's leaves it to the app, so there gpui
//! reports `Decorations::Client` and the window draws what a title bar
//! would give: edges to resize it by, on the sides that aren't tiled, and
//! buttons to minimize, maximize and close it. Elsewhere, and on macOS,
//! these draw nothing.

use crate::theme::*;
use gpui::{prelude::*, *};

/// How far in from the window's edge a drag resizes it.
const EDGE: f32 = 6.;
/// The corners' reach along each side.
const CORNER: f32 = 14.;

/// The resize edges, as children of the window's root (which must be
/// `relative`), drawn over the content.
pub fn edges(window: &Window) -> Vec<AnyElement> {
    let Decorations::Client { tiling } = window.window_decorations() else {
        return vec![];
    };
    let (top, bottom, left, right) = (!tiling.top, !tiling.bottom, !tiling.left, !tiling.right);
    let strip = |id: &'static str, edge: ResizeEdge, cursor: CursorStyle| {
        div().id(id).absolute().cursor(cursor).on_mouse_down(
            MouseButton::Left,
            move |_, window, cx| {
                cx.stop_propagation();
                window.start_window_resize(edge);
            },
        )
    };
    let mut out = vec![];
    if top {
        out.push(
            strip("csd-top", ResizeEdge::Top, CursorStyle::ResizeUpDown)
                .top_0()
                .left(px(CORNER))
                .right(px(CORNER))
                .h(px(EDGE)),
        );
    }
    if bottom {
        out.push(
            strip("csd-bottom", ResizeEdge::Bottom, CursorStyle::ResizeUpDown)
                .bottom_0()
                .left(px(CORNER))
                .right(px(CORNER))
                .h(px(EDGE)),
        );
    }
    if left {
        out.push(
            strip("csd-left", ResizeEdge::Left, CursorStyle::ResizeLeftRight)
                .left_0()
                .top(px(CORNER))
                .bottom(px(CORNER))
                .w(px(EDGE)),
        );
    }
    if right {
        out.push(
            strip("csd-right", ResizeEdge::Right, CursorStyle::ResizeLeftRight)
                .right_0()
                .top(px(CORNER))
                .bottom(px(CORNER))
                .w(px(EDGE)),
        );
    }
    let corner = |id, edge, cursor| strip(id, edge, cursor).size(px(CORNER));
    if top && left {
        out.push(
            corner(
                "csd-tl",
                ResizeEdge::TopLeft,
                CursorStyle::ResizeUpLeftDownRight,
            )
            .top_0()
            .left_0(),
        );
    }
    if top && right {
        out.push(
            corner(
                "csd-tr",
                ResizeEdge::TopRight,
                CursorStyle::ResizeUpRightDownLeft,
            )
            .top_0()
            .right_0(),
        );
    }
    if bottom && left {
        out.push(
            corner(
                "csd-bl",
                ResizeEdge::BottomLeft,
                CursorStyle::ResizeUpRightDownLeft,
            )
            .bottom_0()
            .left_0(),
        );
    }
    if bottom && right {
        out.push(
            corner(
                "csd-br",
                ResizeEdge::BottomRight,
                CursorStyle::ResizeUpLeftDownRight,
            )
            .bottom_0()
            .right_0(),
        );
    }
    out.into_iter().map(IntoElement::into_any_element).collect()
}

/// Minimize, maximize and close, at the window's top right over the tab
/// strip, when the app draws its own decorations.
pub fn controls(window: &Window) -> Option<AnyElement> {
    let Decorations::Client { .. } = window.window_decorations() else {
        return None;
    };
    let maximized = window.is_maximized();
    Some(
        div()
            .absolute()
            .top(px(4.))
            .right(px(8.))
            .flex()
            .gap_1()
            .bg(rgb(pal().tabbar_bg))
            .rounded(px(5.))
            .child(
                icon_button("csd-minimize", "minus")
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(|_, window, _| window.minimize_window()),
            )
            .child(
                icon_button(
                    "csd-maximize",
                    if maximized {
                        "corner-down-right"
                    } else {
                        "maximize-2"
                    },
                )
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(|_, window, _| window.zoom_window()),
            )
            .child(
                icon_button("csd-close", "x")
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(|_, window, cx| crate::quit::request(window, cx)),
            )
            .into_any_element(),
    )
}
