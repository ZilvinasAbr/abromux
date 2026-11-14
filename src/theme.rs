#![allow(unexpected_cfgs)] // objc's msg_send!

//! Colours and small shared widgets: cmux's layout in Claude's warm neutrals
//! with the terracotta orange accent, light or dark.
//!
//! The app's appearance setting (`app.appearance`: system, light or dark)
//! sets macOS's appearance for the app ([`apply_appearance`]); the windows'
//! effective appearance then picks the palette ([`set_dark`]), so "system"
//! follows macOS as it changes.

use gpui::{prelude::*, *};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

/// The UI's colours, as `0xrrggbb` (or `0xrrggbbaa` where noted).
pub struct Palette {
    /// The sidebar, over the window's blur (rgba).
    pub sidebar_bg: u32,
    pub pane_bg: u32,
    pub tabbar_bg: u32,
    pub border: u32,
    pub input_bg: u32,
    pub text: u32,
    pub muted: u32,
    pub accent: u32,
    pub accent_bright: u32,
    /// A row or button under the pointer (rgba).
    pub hover: u32,
    /// Menus and popovers drawn by gpui-component (context menus, dropdowns).
    pub menu_bg: u32,
    pub menu_border: u32,
    /// A menu row under the pointer or keyboard.
    pub menu_hover: u32,
    /// Fill for where a dragged tab would land (rgba).
    pub drop_tint: u32,
    /// The colour [`tint`] lays over the background: white on dark, black
    /// on light (rgb).
    pub overlay: u32,
    /// A dimming layer over content, e.g. an inactive pane (rgba).
    pub scrim: u32,
    /// An inline text field (renaming in place), also on an accent row,
    /// and its border (rgba).
    pub field_bg: u32,
    pub field_border: u32,
    /// A group of rows on a page, e.g. the settings' cards (rgba).
    pub card_bg: u32,
    /// The shortcut hints shown while ⌘/⌃ is held, and their border (rgba).
    pub pill_bg: u32,
    pub pill_border: u32,
}

pub const DARK: Palette = Palette {
    sidebar_bg: 0x262624f4,
    pane_bg: 0x1a1918,
    tabbar_bg: 0x1f1e1d,
    border: 0x3a3935,
    input_bg: 0x30302e,
    text: 0xfaf9f5,
    muted: 0x9c9a92,
    accent: 0xc96442,
    accent_bright: 0xd97757,
    hover: 0xffffff10,
    menu_bg: 0x2c2b28,
    menu_border: 0x45433e,
    menu_hover: 0x3e3d39,
    drop_tint: 0xc9644233,
    overlay: 0xffffff,
    scrim: 0x0000004a,
    field_bg: 0x0000004d,
    field_border: 0xffffff59,
    card_bg: 0xffffff06,
    pill_bg: 0x3a3c42f2,
    pill_border: 0xffffff4d,
};

pub const LIGHT: Palette = Palette {
    sidebar_bg: 0xefede5f0,
    pane_bg: 0xfaf9f5,
    tabbar_bg: 0xf3f1ea,
    border: 0xdedcd1,
    input_bg: 0xffffff,
    text: 0x1f1e1d,
    muted: 0x6f6d66,
    accent: 0xc96442,
    accent_bright: 0xc96442,
    hover: 0x0000000c,
    menu_bg: 0xffffff,
    menu_border: 0xdedcd1,
    menu_hover: 0xf0eee6,
    drop_tint: 0xc9644233,
    overlay: 0x000000,
    scrim: 0xffffff59,
    field_bg: 0xffffffeb,
    field_border: 0x00000033,
    card_bg: 0xffffffc0,
    pill_bg: 0xfffffff2,
    pill_border: 0x00000029,
};

static IS_DARK: AtomicBool = AtomicBool::new(true);

/// The palette in use.
pub fn pal() -> &'static Palette {
    if is_dark() { &DARK } else { &LIGHT }
}

pub fn is_dark() -> bool {
    IS_DARK.load(Relaxed)
}

/// `alpha` of the palette's overlay colour (white on dark, black on light):
/// for hover and selection fills over the background. Black shows more than
/// white at the same alpha, so it's lightened to look alike.
pub fn tint(alpha: u8) -> Rgba {
    let a = if is_dark() {
        alpha
    } else {
        (alpha as u32 * 2 / 3) as u8
    };
    rgba(pal().overlay << 8 | a as u32)
}

/// The app's appearance setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Appearance {
    /// Follow macOS.
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

/// Set macOS's appearance for the app (`NSApp.appearance`): its windows,
/// their blur, menus and web views follow it. System clears it, so they
/// follow macOS's. The palette follows in [`sync`], when the windows
/// report their new appearance.
pub fn apply_appearance(a: Appearance) {
    use objc::{class, msg_send, runtime::Object, sel, sel_impl};
    #[link(name = "AppKit", kind = "framework")]
    unsafe extern "C" {
        static NSAppearanceNameAqua: *mut Object;
        static NSAppearanceNameDarkAqua: *mut Object;
    }
    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let appearance: *mut Object = match a {
            Appearance::System => std::ptr::null_mut(),
            Appearance::Light => {
                msg_send![class!(NSAppearance), appearanceNamed: NSAppearanceNameAqua]
            }
            Appearance::Dark => {
                msg_send![class!(NSAppearance), appearanceNamed: NSAppearanceNameDarkAqua]
            }
        };
        let _: () = msg_send![app, setAppearance: appearance];
    }
}

/// Is this appearance a dark one?
pub fn appearance_is_dark(a: WindowAppearance) -> bool {
    matches!(a, WindowAppearance::Dark | WindowAppearance::VibrantDark)
}

/// Use the light or dark palette, here and in gpui-component's widgets.
/// Returns whether it changed.
pub fn set_dark(dark: bool, cx: &mut App) -> bool {
    let changed = IS_DARK.swap(dark, Relaxed) != dark;
    style_components(cx);
    changed
}

/// Make the app's setting macOS's appearance for the app, and follow it.
pub fn apply(a: Appearance, cx: &mut App) {
    apply_appearance(a);
    sync(appearance_is_dark(cx.window_appearance()), cx);
}

/// Follow a window's appearance (it changed, or the setting did): the
/// palette, the terminals' colors (a Ghostty `theme = light:A,dark:B` has
/// one for each), and every window redrawn.
pub fn sync(dark: bool, cx: &mut App) {
    if !set_dark(dark, cx) {
        return;
    }
    cx.set_global(crate::config::Config::load());
    cx.refresh_windows();
}

/// Keep the palette following `window`'s appearance as macOS's changes.
pub fn follow(window: &Window) {
    window
        .observe_window_appearance(|window, cx| sync(appearance_is_dark(window.appearance()), cx))
        .detach();
}

/// gpui-component's theme in the palette: its own light and dark themes are
/// cold greys, so its menus, popovers and fields get our warm neutrals.
fn style_components(cx: &mut App) {
    use gpui_component::{Theme, ThemeMode};
    let mode = if is_dark() {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    };
    Theme::change(mode, None, cx);
    let p = pal();
    let theme = Theme::global_mut(cx);
    theme.background = rgba(0x00000000).into();
    theme.font_family = ".AppleSystemUIFont".into();
    theme.mono_font_family = crate::terminal::FONT_FAMILY.into();
    theme.popover = rgb(p.menu_bg).into();
    theme.popover_foreground = rgb(p.text).into();
    theme.foreground = rgb(p.text).into();
    theme.muted_foreground = rgb(p.muted).into();
    theme.border = rgb(p.menu_border).into();
    theme.accent = rgb(p.menu_hover).into();
    theme.accent_foreground = rgb(p.text).into();
}

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
    let step = START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis()
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
        .text_color(rgb(pal().accent_bright))
        .child(CLAUDE_FRAMES[i])
}

pub fn icon_button(id: impl Into<ElementId>, name: &str) -> Stateful<Div> {
    tinted_icon_button(id, name, pal().muted)
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
        .text_color(rgb(pal().muted))
        .cursor_pointer()
        .hover(|s| s.bg(rgba(pal().hover)).text_color(rgb(pal().text)))
        .active(|s| s.opacity(0.7))
}
