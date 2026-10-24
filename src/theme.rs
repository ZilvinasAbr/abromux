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

pub fn icon(name: &str) -> Svg {
    svg().path(format!("icons/{name}.svg")).size(px(14.)).flex_none()
}

pub fn icon_button(id: impl Into<ElementId>, name: &str) -> Stateful<Div> {
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
        .child(icon(name).text_color(rgb(MUTED)))
}
