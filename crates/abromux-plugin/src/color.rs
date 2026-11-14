//! Hex colours, read the same way everywhere in abromux and its plugins:
//! `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa`, with the `#` optional. The
//! parsers are `abromux_core::color`'s; [`parse`] makes one a gpui colour.

pub use abromux_core::color::{parse_rgb, parse_rgba};
use gpui::Rgba;

/// The colour with its alpha, for gpui.
pub fn parse(s: &str) -> Option<Rgba> {
    parse_rgba(s).map(gpui::rgba)
}
