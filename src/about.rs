//! The About window, laid out like cmux's: icon, name, a one-line
//! description, version/build/commit, and Docs/GitHub/Licenses buttons.
//! There's only ever one; opening it again brings it to the front.

use crate::{channel, theme::*};
use gpui::{prelude::*, *};
use std::{cell::Cell, sync::Arc};

const REPO: &str = env!("CARGO_PKG_REPOSITORY");
const DESCRIPTION: &str =
    "A cmux-style terminal and browser workspace\nfor macOS, built on libghostty.";

thread_local! {
    static OPEN: Cell<Option<AnyWindowHandle>> = const { Cell::new(None) };
}

actions!(about, [CloseAbout]);

pub fn show(cx: &mut App) {
    if let Some(handle) = OPEN.get()
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }
    let bounds = Bounds::centered(None, size(px(380.), px(470.)), cx);
    let handle = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some(format!("About {}", channel::app_name()).into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(14.), px(13.))),
            }),
            window_background: WindowBackgroundAppearance::Blurred,
            is_resizable: false,
            is_minimizable: false,
            ..Default::default()
        },
        |window, cx| {
            cx.new(|cx| {
                let focus = cx.focus_handle();
                window.focus(&focus);
                About { focus }
            })
        },
    );
    if let Ok(handle) = handle {
        OPEN.set(Some(handle.into()));
        cx.activate(true);
    }
}

struct About {
    focus: FocusHandle,
}

impl Render for About {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let (build, commit) = (env!("ABROMUX_BUILD"), env!("ABROMUX_COMMIT"));
        let version = env!("ABROMUX_VERSION");
        let version = version.split('+').next().unwrap_or(version);
        let dirty = env!("ABROMUX_VERSION").ends_with("-dirty");
        let commit_value = div()
            .id("commit")
            .text_color(rgb(ACCENT_BRIGHT))
            .cursor_pointer()
            .hover(|s| s.underline())
            .child(if dirty {
                format!("{commit}-dirty")
            } else {
                commit.to_string()
            })
            .on_click(move |_, _, cx| cx.open_url(&format!("{REPO}/commit/{commit}")));
        div()
            .key_context("About")
            .track_focus(&self.focus)
            .on_action(|_: &CloseAbout, window, _| window.remove_window())
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .pt(px(48.))
            .bg(rgba(SIDEBAR_BG))
            .font_family(".AppleSystemUIFont")
            .text_color(rgb(TEXT))
            .child(
                img(Arc::new(Image::from_bytes(
                    ImageFormat::Png,
                    crate::icon::png().to_vec(),
                )))
                .size(px(128.)),
            )
            .child(
                div()
                    .mt(px(14.))
                    .text_size(px(26.))
                    .font_weight(FontWeight::BOLD)
                    .child(channel::app_name()),
            )
            .child(
                div()
                    .mt(px(8.))
                    .px(px(24.))
                    .text_size(px(13.))
                    .text_center()
                    .text_color(rgb(MUTED))
                    .child(DESCRIPTION),
            )
            .child(
                div()
                    .mt(px(24.))
                    .flex()
                    .gap(px(10.))
                    .text_size(px(13.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_end()
                            .gap(px(4.))
                            .child("Version")
                            .when(!build.is_empty(), |d| d.child("Build"))
                            .when(!commit.is_empty(), |d| d.child("Commit")),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .font_family(crate::terminal::FONT_FAMILY)
                            .text_color(rgb(MUTED))
                            .child(version.to_string())
                            .when(!build.is_empty(), |d| d.child(build))
                            .when(!commit.is_empty(), |d| d.child(commit_value)),
                    ),
            )
            .child(
                div()
                    .mt(px(28.))
                    .flex()
                    .gap(px(10.))
                    .child(button("Docs", format!("{REPO}/blob/main/docs/guide.md")))
                    .child(button("GitHub", REPO.to_string()))
                    .child(button("Licenses", format!("{REPO}/blob/main/LICENSE"))),
            )
    }
}

fn button(label: &'static str, url: String) -> Stateful<Div> {
    div()
        .id(label)
        .px(px(14.))
        .py(px(5.))
        .rounded(px(6.))
        .bg(rgba(HOVER))
        .text_size(px(13.))
        .cursor_pointer()
        .hover(|s| s.bg(rgba(0xffffff1c)))
        .active(|s| s.opacity(0.7))
        .child(label)
        .on_click(move |_, _, cx| cx.open_url(&url))
}
