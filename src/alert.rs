//! Confirmation sheets, as cmux shows them: a message, a "Don't ask again"
//! checkbox, and Cancel next to the action. A ticked box is kept in
//! `abromux_platform::prefs` under the sheet's key, and only when the user
//! confirms.
//!
//! The sheet is drawn by gpui, under the window's titlebar, by the
//! [`Layer`] each window's view sits in. Like a native sheet it's modal:
//! it takes the keyboard (↩ confirms, ⎋ cancels) and the mouse, and the
//! window's shortcuts wait until it's gone.

use crate::theme::*;
use futures::channel::oneshot;
use gpui::{prelude::*, *};

/// The keys the native sheets kept in NSUserDefaults, copied into the
/// prefs file once ([`abromux_platform::prefs::import_once`]).
pub const SKIP_KEYS: [&str; 2] = [crate::quit::SKIP_KEY, crate::SKIP_CLOSE_CLAUDE_KEY];

/// Where a sheet hangs from: just under the titlebar.
const TOP: f32 = crate::TITLEBAR_H;
const WIDTH: f32 = 280.;

pub struct Confirm<'a> {
    pub message: &'a str,
    pub info: &'a str,
    /// The action's button, e.g. "Close".
    pub button: &'a str,
    /// The "Don't ask again" checkbox's label; "" for no checkbox.
    pub suppress: &'a str,
    /// Prefs key that turns the sheet off; "" for a sheet that always asks.
    pub skip_key: &'a str,
}

/// The sheet on screen, at most one in the app.
struct Open {
    window: AnyWindowHandle,
    message: SharedString,
    info: SharedString,
    button: SharedString,
    suppress: SharedString,
    skip_key: String,
    ticked: bool,
    tx: Option<oneshot::Sender<bool>>,
    focus: FocusHandle,
    /// What had focus before, to give it back.
    restore: Option<FocusHandle>,
}

#[derive(Default)]
struct Sheets {
    open: Option<Open>,
    /// Windows with a [`Layer`], which can show a sheet.
    layered: Vec<AnyWindowHandle>,
}

impl Global for Sheets {}

/// Is a sheet up (in any window)? Pages show as bitmaps meanwhile, so the
/// sheet isn't hidden behind them.
pub fn is_open(cx: &App) -> bool {
    cx.try_global::<Sheets>().is_some_and(|s| s.open.is_some())
}

/// Ask on a sheet over `window` (or, if that window can't show one, over a
/// window that can). Resolves to true to go ahead, at once if the user
/// turned this sheet off. While another sheet is up the request is dropped
/// (the receiver errors).
pub fn confirm(window: &mut Window, cx: &mut App, c: Confirm) -> oneshot::Receiver<bool> {
    let (tx, rx) = oneshot::channel();
    if !c.skip_key.is_empty() && abromux_platform::prefs::bool(c.skip_key) {
        let _ = tx.send(true);
        return rx;
    }
    let sheets = cx.default_global::<Sheets>();
    if sheets.open.is_some() {
        return rx;
    }
    let here = window.window_handle();
    let target = if sheets.layered.contains(&here) {
        here
    } else if let Some(&w) = sheets.layered.first() {
        w
    } else {
        let _ = tx.send(true);
        return rx;
    };
    let open = |restore, focus| Open {
        window: target,
        message: c.message.to_string().into(),
        info: c.info.to_string().into(),
        button: c.button.to_string().into(),
        suppress: c.suppress.to_string().into(),
        skip_key: c.skip_key.to_string(),
        ticked: false,
        tx: Some(tx),
        focus,
        restore,
    };
    if target == here {
        let focus = cx.focus_handle();
        let restore = window.focused(cx);
        window.focus(&focus);
        cx.global_mut::<Sheets>().open = Some(open(restore, focus));
        window.refresh();
    } else {
        let _ = target.update(cx, |_, window, cx| {
            window.activate_window();
            let focus = cx.focus_handle();
            let restore = window.focused(cx);
            window.focus(&focus);
            cx.global_mut::<Sheets>().open = Some(open(restore, focus));
            window.refresh();
        });
    }
    rx
}

/// Close the sheet with the user's answer.
fn answer(ok: bool, window: &mut Window, cx: &mut App) {
    let Some(mut open) = cx.global_mut::<Sheets>().open.take() else {
        return;
    };
    if ok && open.ticked && !open.skip_key.is_empty() {
        abromux_platform::prefs::set_bool(&open.skip_key, true);
    }
    if let Some(restore) = open.restore.take() {
        window.focus(&restore);
    }
    if let Some(tx) = open.tx.take() {
        let _ = tx.send(ok);
    }
    window.refresh();
}

/// A window's content with room for a sheet over it: wrap each window's
/// view in one (inside gpui-component's `Root`). The sheet sits beside the
/// view rather than in it, so while it has the keyboard none of the view's
/// shortcuts can act.
pub struct Layer {
    view: AnyView,
}

impl Layer {
    pub fn new(view: impl Into<AnyView>, window: &mut Window, cx: &mut App) -> Self {
        cx.default_global::<Sheets>()
            .layered
            .push(window.window_handle());
        Self { view: view.into() }
    }
}

impl Render for Layer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let here = window.window_handle();
        let sheet = cx
            .try_global::<Sheets>()
            .and_then(|s| s.open.as_ref())
            .filter(|o| o.window == here)
            .map(render_sheet);
        div()
            .size_full()
            .relative()
            .child(self.view.clone())
            .children(sheet)
    }
}

fn render_sheet(open: &Open) -> AnyElement {
    let ticked = open.ticked;
    let checkbox = div()
        .id("alert-suppress")
        .flex()
        .items_center()
        .gap(px(6.))
        .cursor_pointer()
        .on_click(|_, window, cx| {
            if let Some(o) = cx.global_mut::<Sheets>().open.as_mut() {
                o.ticked = !o.ticked;
            }
            window.refresh();
        })
        .child(
            div()
                .size(px(14.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(3.))
                .border_1()
                .when(ticked, |d| {
                    d.bg(rgb(pal().accent))
                        .border_color(rgb(pal().accent))
                        .child(icon("check").size(px(10.)).text_color(rgb(0xffffff)))
                })
                .when(!ticked, |d| {
                    d.bg(rgb(pal().input_bg)).border_color(rgb(pal().border))
                }),
        )
        .child(open.suppress.clone());
    let button = |id: &'static str, label: SharedString, primary: bool, ok: bool| {
        div()
            .id(id)
            .flex_1()
            .flex()
            .justify_center()
            .py(px(5.))
            .rounded(px(6.))
            .text_size(px(13.))
            .when(primary, |d| {
                d.bg(rgb(pal().accent)).text_color(rgb(0xffffff))
            })
            .when(!primary, |d| d.bg(rgba(pal().hover)))
            .cursor_pointer()
            .hover(|s| s.opacity(0.85))
            .active(|s| s.opacity(0.7))
            .on_click(move |_, window, cx| answer(ok, window, cx))
            .child(label)
    };
    let card = div()
        .id("alert-sheet")
        .key_context("AlertSheet")
        .track_focus(&open.focus)
        .on_key_down(|ev, window, cx| {
            let k = &ev.keystroke;
            let plain = !k.modifiers.modified();
            match k.key.as_str() {
                "enter" if plain => answer(true, window, cx),
                "escape" if plain => answer(false, window, cx),
                "." if k.modifiers.platform && !k.modifiers.shift => answer(false, window, cx),
                _ => return,
            }
            cx.stop_propagation();
        })
        .w(px(WIDTH))
        .p(px(16.))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(10.))
        .rounded(px(10.))
        .bg(rgb(pal().tabbar_bg))
        .border_1()
        .border_color(rgb(pal().border))
        .shadow_lg()
        .text_size(px(12.))
        .text_color(rgb(pal().text))
        .child(
            img(std::sync::Arc::new(Image::from_bytes(
                ImageFormat::Png,
                crate::icon::png().to_vec(),
            )))
            .size(px(56.)),
        )
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::BOLD)
                .text_center()
                .child(open.message.clone()),
        )
        .when(!open.info.is_empty(), |d| {
            d.child(
                div()
                    .text_size(px(11.5))
                    .text_center()
                    .child(open.info.clone()),
            )
        })
        .when(!open.suppress.is_empty(), |d| {
            d.child(div().w_full().child(checkbox))
        })
        .child(
            div()
                .w_full()
                .flex()
                .gap(px(8.))
                .child(button("alert-cancel", "Cancel".into(), false, false))
                .child(button("alert-ok", open.button.clone(), true, true)),
        );
    // The whole window takes the mouse, so nothing under the sheet can be
    // clicked while it's up.
    div()
        .id("alert-backdrop")
        .absolute()
        .inset_0()
        .occlude()
        .flex()
        .justify_center()
        .child(div().mt(px(TOP)).child(card))
        .into_any_element()
}
