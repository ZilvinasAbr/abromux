//! Colours and small shared widgets: cmux's dark layout in Claude's warm
//! neutrals with the terracotta orange accent.

use gpui::{prelude::*, *};

pub const SIDEBAR_BG: u32 = 0x262624f4;
pub const PANE_BG: u32 = 0x1a1918;
pub const TABBAR_BG: u32 = 0x1f1e1d;
pub const BORDER: u32 = 0x3a3935;
pub const INPUT_BG: u32 = 0x30302e;
pub const TEXT: u32 = 0xfaf9f5;
pub const MUTED: u32 = 0x9c9a92;
pub const ACCENT: u32 = 0xc96442;
pub const ACCENT_BRIGHT: u32 = 0xd97757;
pub const HOVER: u32 = 0xffffff10;
/// Fill for where a dragged tab would land.
pub const DROP_TINT: u32 = 0xc9644233;

pub fn icon(name: &str) -> Svg {
    svg()
        .path(format!("icons/{name}.svg"))
        .size(px(14.))
        .flex_none()
}

/// Claude Code's own working animation: its star grows through these and
/// shrinks back.
const CLAUDE_FRAMES: [&str; 6] = ["·", "✢", "✳", "✶", "✻", "✽"];
/// How long each frame shows.
pub const CLAUDE_FRAME_MS: u64 = 120;

/// Claude Code's star at this moment of its animation, in Claude's orange.
/// Whoever shows it redraws while Claude works (`AbroMux::tick_spinner`).
pub fn claude_spinner(size: Pixels) -> Div {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let n = CLAUDE_FRAMES.len();
    let step = START.get_or_init(std::time::Instant::now).elapsed().as_millis()
        / CLAUDE_FRAME_MS as u128;
    // 0 1 2 3 4 5 4 3 2 1 0 …
    let i = (step % (2 * n as u128 - 2)) as usize;
    let i = if i < n { i } else { 2 * n - 2 - i };
    div()
        .size(size)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .text_size(size)
        .line_height(size)
        .text_color(rgb(ACCENT_BRIGHT))
        .child(CLAUDE_FRAMES[i])
}

pub fn icon_button(id: impl Into<ElementId>, name: &str) -> Stateful<Div> {
    tinted_icon_button(id, name, MUTED)
}

/// An [`icon_button`] whose icon is `color` rather than muted.
pub fn tinted_icon_button(id: impl Into<ElementId>, name: &str, color: u32) -> Stateful<Div> {
    button_frame(id).child(icon(name).text_color(rgb(color)))
}

/// An [`icon_button`] without its icon, for a child of the caller's own.
pub fn button_frame(id: impl Into<ElementId>) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(22.))
        .rounded(px(5.))
        .text_color(rgb(MUTED))
        .cursor_pointer()
        .hover(|s| s.bg(rgba(HOVER)).text_color(rgb(TEXT)))
        .active(|s| s.opacity(0.7))
}
