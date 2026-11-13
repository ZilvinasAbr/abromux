//! GitLab's dark theme, as its web UI computes it (read off GitLab 19.5 with
//! the dev box's `bob` in dark mode), and the small pieces built from it.

use crate::highlight::Tok;
use gpui::{prelude::*, *};

/// How much bigger than GitLab's own sizes the tab is drawn (⌘= / ⌘- /
/// ⌘0), for every GitLab tab, kept across restarts in the cache's folder.
static ZOOM: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The zoom levels ⌘= and ⌘- step through, as a browser's.
pub const ZOOMS: [f32; 13] = [
    0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0,
];

fn zoom_file() -> Option<std::path::PathBuf> {
    Some(crate::cache::root()?.join("zoom"))
}

pub fn zoom() -> f32 {
    use std::sync::atomic::Ordering::Relaxed;
    match ZOOM.load(Relaxed) {
        // Not read yet: what was saved, else GitLab's size.
        0 => {
            let saved = zoom_file()
                .and_then(|f| std::fs::read_to_string(f).ok())
                .and_then(|t| t.trim().parse::<f32>().ok())
                .filter(|z| (ZOOMS[0]..=ZOOMS[ZOOMS.len() - 1]).contains(z))
                .unwrap_or(1.0);
            ZOOM.store(saved.to_bits(), Relaxed);
            saved
        }
        bits => f32::from_bits(bits),
    }
}

/// Draw every GitLab tab at `z` times GitLab's sizes (within `ZOOMS`).
pub fn set_zoom(z: f32) {
    let z = z.clamp(ZOOMS[0], ZOOMS[ZOOMS.len() - 1]);
    ZOOM.store(z.to_bits(), std::sync::atomic::Ordering::Relaxed);
    if let Some(f) = zoom_file() {
        let _ = std::fs::write(f, format!("{z}\n"));
    }
}

/// The zoom level after `z`, a step in (`up`) or out.
pub fn zoom_step(z: f32, up: bool) -> f32 {
    let next = if up {
        ZOOMS.iter().copied().find(|s| *s > z + 0.001)
    } else {
        ZOOMS.iter().rev().copied().find(|s| *s < z - 0.001)
    };
    next.unwrap_or(z)
}

/// Pixels at the tab's zoom: every size in the plugin goes through this,
/// so the whole tab scales together. (It shadows gpui's `px`; a size that
/// must not scale, like a measured one, is `gpui::px`.)
pub fn px(v: f32) -> Pixels {
    gpui::px(v * zoom())
}

/// Draws `child` with rems scaled too (`gap_2`, `px_4`, gpui-component's
/// text fields and markdown), as `px` scales the rest.
pub struct Zoomed {
    child: AnyElement,
    rem: Pixels,
}

impl Zoomed {
    pub fn new(child: impl IntoElement, window: &Window) -> Self {
        Self {
            child: child.into_any_element(),
            rem: window.rem_size() * zoom(),
        }
    }
}

impl IntoElement for Zoomed {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Zoomed {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (
            window.with_rem_size(Some(self.rem), |window| {
                self.child.request_layout(window, cx)
            }),
            (),
        )
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_rem_size(Some(self.rem), |window| self.child.prepaint(window, cx));
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_rem_size(Some(self.rem), |window| self.child.paint(window, cx));
    }
}

pub fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

pub fn ca(hex_rgba: u32) -> Hsla {
    rgba(hex_rgba).into()
}

pub const BG: u32 = 0x18171d;
/// File headers, thread headers, cards.
pub const CARD: u32 = 0x28272d;
/// Raised cards (approval widget, suggestion header), the timeline's line.
pub const RAISED: u32 = 0x3a383f;
pub const BORDER: u32 = 0x4c4b51;
pub const TEXT: u32 = 0xececef;
pub const MUTED: u32 = 0xbfbfc3;
pub const HEADING: u32 = 0xffffff;
pub const LINK: u32 = 0x9dc8f2;
/// GitLab's focus ring.
pub const FOCUS: u32 = 0x428fdc;
pub const GREEN: u32 = 0x35b597;
pub const RED: u32 = 0xfc7988;
pub const ORANGE: u32 = 0xe9be74;

pub const CODE_BG: u32 = 0x1d1f21;
/// The line whose number was clicked: its code, and its number cells.
pub const SELECTED_BG: u32 = 0x7d2d0e;
pub const SELECTED_NUM_BG: u32 = 0x483698;
/// A side-by-side cell with no line.
pub const EMPTY_BG: u32 = 0x232428;
pub const CODE_TEXT: u32 = 0xc5c8c6;
pub const LINE_NUM: u32 = 0xffffff4d;
pub const ADD_BG: u32 = 0x233726;
pub const ADD_NUM: u32 = 0x627165;
pub const ADD_WORD: u32 = 0x33ff3333;
pub const DEL_BG: u32 = 0x4b2526;
pub const DEL_NUM: u32 = 0x806565;
pub const DEL_WORD: u32 = 0xff333340;
pub const HUNK_BG: u32 = 0x343638;
pub const HUNK_TEXT: u32 = 0xc1bfbe;

/// GitLab's mono and diff text: 12.5px on 20px lines.
pub const CODE_SIZE: f32 = 12.5;
pub const CODE_LINE: f32 = 20.;
/// Line number columns.
pub const NUM_W: f32 = 50.;

/// Code text: the mono font at GitLab's size, without ligatures (GitLab
/// Mono has none; a terminal font may turn `->` into an arrow).
pub fn code(mut d: Div, mono: SharedString) -> Div {
    d.text_style()
        .get_or_insert_with(Default::default)
        .font_features = Some(FontFeatures::disable_ligatures());
    d.font_family(mono)
        .text_size(px(CODE_SIZE))
        .line_height(px(CODE_LINE))
}

/// Markdown (descriptions, notes) with GitLab's code blocks: the code
/// background, a 1px border, 12px padding.
pub fn md_style() -> gpui_component::text::TextViewStyle {
    let code_block = StyleRefinement::default()
        .bg(c(CODE_BG))
        .border_1()
        .border_color(c(BORDER))
        .rounded(px(4.))
        .p(px(12.));
    // GitLab's headings: 1.75, 1.5 and 1.25 times the text, then the text's size.
    let heading = |level: u8, base: Pixels| {
        base * match level {
            1 => 1.75,
            2 => 1.5,
            3 => 1.25,
            _ => 1.0,
        }
    };
    gpui_component::text::TextViewStyle {
        code_block,
        is_dark: true,
        heading_base_font_size: px(14.),
        heading_font_size: Some(std::sync::Arc::new(heading)),
        ..Default::default()
    }
}

pub fn token(t: Tok) -> Hsla {
    c(match t {
        Tok::Keyword => 0xb294bb,
        Tok::String => 0xb5bd68,
        Tok::Comment => 0x969896,
        Tok::Number => 0xde935f,
        Tok::Function => 0x81a2be,
        Tok::Namespace => 0xf0c674,
        Tok::Operator | Tok::Macro | Tok::Heading => 0x8abeb7,
        Tok::Variable => 0xcc6666,
        Tok::Type => 0xf0c674,
        Tok::Tag => 0xcc6666,
        Tok::Attribute => 0xde935f,
    })
}

/// A pill: GitLab's `gl-badge`.
pub fn badge(text: impl Into<SharedString>, bg: Hsla, fg: Hsla) -> Div {
    div()
        .flex()
        .items_center()
        .gap_1()
        .px(px(6.))
        .py(px(2.))
        .rounded_full()
        .bg(bg)
        .text_color(fg)
        .text_size(px(12.))
        .line_height(px(16.))
        .child(text.into())
}

/// "4 hours ago", with the exact time on hover as in GitLab.
pub fn when(iso: &str) -> Stateful<Div> {
    let exact: SharedString = crate::time::exact(iso).into();
    div()
        .id(SharedString::from(format!("t-{iso}")))
        .child(crate::time::ago(iso, crate::time::now()))
        .tooltip(move |window, cx| {
            gpui_component::tooltip::Tooltip::new(exact.clone()).build(window, cx)
        })
}

/// A hover hint, as GitLab's buttons have.
pub fn hint(text: &'static str) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    move |window, cx| gpui_component::tooltip::Tooltip::new(text).build(window, cx)
}

/// A pipeline's status icon in GitLab's colors.
pub fn pipeline_icon(p: &crate::types::Pipeline, size: f32) -> Svg {
    use crate::types::PipelineTone::*;
    match p.label().1 {
        Good => icon("check-circle", size, c(GREEN)),
        // abromux's own icon for the failed one.
        Bad => app_icon("circle-x", size, c(RED)),
        Busy => icon("loader", size, c(0x63a6e9)),
        Waiting => icon("status-pending", size, c(0xe9a03b)),
        Neutral => icon("status-canceled", size, c(MUTED)),
    }
}

/// `#1068bf` → a color; grey for anything else.
pub fn hex(s: &str) -> Hsla {
    abromux_plugin::color::parse_rgb(s).map_or(c(0x737278), c)
}

/// A label as GitLab 19.5 draws it: a pill in its color with its text
/// color, scoped ones (`priority::high`) included.
pub fn label_pill(l: &crate::types::Label) -> Div {
    let bg = if l.color.is_empty() {
        c(RAISED)
    } else {
        hex(&l.color)
    };
    let fg = if l.text_color.is_empty() {
        c(TEXT)
    } else {
        hex(&l.text_color)
    };
    div()
        .px(px(6.))
        .rounded(px(6.))
        .bg(bg)
        .text_color(fg)
        .text_size(px(12.))
        .line_height(px(18.))
        .child(l.name.clone())
}

/// The grey count next to a tab's name.
pub fn count_badge(n: usize) -> Div {
    badge(n.to_string(), c(0xa4a3a8), c(BG))
}

/// A branch name: `rate-limiter`.
pub fn branch(name: &str, mono: SharedString) -> Div {
    div()
        .px(px(4.))
        .rounded(px(4.))
        .bg(c(0x254581))
        .text_color(c(0xc8e0f9))
        .font_family(mono)
        .text_size(px(12.))
        .line_height(px(18.))
        .child(name.to_string())
}

/// `@bob` in a system note: orange for you, blue for others.
pub fn mention(name: &str, me: bool) -> Div {
    let (bg, fg) = if me {
        (0x7d2d0e, 0xfada9c)
    } else {
        (0x254581, 0xc8e0f9)
    };
    div()
        .px(px(2.))
        .rounded(px(4.))
        .bg(c(bg))
        .text_color(c(fg))
        .child(name.to_string())
}

/// One of the plugin's icons (assets/), tinted.
pub fn icon(name: &str, size: f32, color: Hsla) -> Svg {
    svg()
        .path(SharedString::from(format!("plugins/gitlab/{name}.svg")))
        .size(px(size))
        .flex_none()
        .text_color(color)
}

/// One of abromux's own icons (`chevron-down`, `folder`).
pub fn app_icon(name: &str, size: f32, color: Hsla) -> Svg {
    svg()
        .path(SharedString::from(format!("icons/{name}.svg")))
        .size(px(size))
        .flex_none()
        .text_color(color)
}

/// The file browser's icon color for a path, after GitLab's file-type
/// icons (Rust orange, Markdown blue, …); grey for the rest.
pub fn file_color(path: &str) -> Hsla {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    c(match ext.as_deref() {
        Some("rs") => 0xe37933,
        Some("md" | "markdown" | "txt" | "rst") => 0x63a6e9,
        Some("csv" | "tsv" | "xls" | "xlsx") => 0x52b87a,
        Some("js" | "mjs" | "cjs" | "jsx") => 0xe9be74,
        Some("ts" | "tsx") => 0x3178c6,
        Some("py") => 0x4b8bbe,
        Some("go") => 0x00add8,
        Some("rb") => 0xd2584c,
        Some("json" | "yml" | "yaml" | "toml") => 0xc6a35c,
        Some("css" | "scss") => 0x9f7fd3,
        Some("html" | "vue") => 0xe5734f,
        Some("png" | "jpg" | "jpeg" | "gif" | "svg" | "webp") => 0x35b597,
        Some("sh" | "bash" | "zsh") => 0x89c46a,
        _ => MUTED,
    })
}

/// A file's icon in the tree, as GitLab picks one by type: a picture for
/// images, the Markdown mark, else a page; colored by `file_color`.
pub fn file_icon(path: &str, size: f32) -> Svg {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    let name = match ext.as_deref() {
        Some("png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "ico" | "bmp") => "file-image",
        Some("md" | "markdown") => "markdown",
        _ => "file",
    };
    icon(name, size, file_color(path))
}

/// GitLab's loading spinner, centered with some room around it.
pub fn spinner(id: &'static str) -> Div {
    div()
        .flex()
        .justify_center()
        .py_6()
        .child(icon("loader", 24., c(MUTED)).with_animation(
            id,
            Animation::new(std::time::Duration::from_millis(800)).repeat(),
            |s, t| s.with_transformation(Transformation::rotate(percentage(t))),
        ))
}

/// GitLab's "Viewed"-style checkbox.
pub fn checkbox(on: bool) -> Div {
    div()
        .size(px(16.))
        .rounded(px(4.))
        .border_1()
        .border_color(c(if on { 0x63a6e9 } else { 0x89888d }))
        .when(on, |d| d.bg(c(0x63a6e9)))
        .flex()
        .items_center()
        .justify_center()
        .when(on, |d| d.child(app_icon("check", 12., c(BG))))
}
