//! The updater's pill in the titlebar and its popover (`crate::update` has
//! the checks and installs they show and start): what it's doing, the
//! release's version, size and date, and its buttons.

use crate::{
    channel,
    theme::*,
    update::{self, State, check, dismiss, entity, install, skip, visible},
};
use gpui::{prelude::*, *};
use std::time::Duration;

const REPO: &str = env!("CARGO_PKG_REPOSITORY");
const POPOVER_W: f32 = 300.;

fn percent(got: u64, total: u64) -> Option<u64> {
    (total > 0).then(|| (got.min(total) * 100) / total)
}

fn pill_text(state: &State) -> String {
    match state {
        State::Idle => String::new(),
        State::Checking { .. } => "Checking for Updates…".into(),
        State::Available(r) => format!("Update Available: {}", r.version),
        State::NotFound => "No Updates Available".into(),
        State::Error(f) => f.title.into(),
        State::Downloading { release, got } => match percent(*got, release.size) {
            Some(p) => format!("Downloading: {p}%"),
            None => "Downloading…".into(),
        },
        State::Preparing(_) => "Preparing…".into(),
        State::Installing(_) => "Installing…".into(),
    }
}

/// The pill, while the updater has something to say. A click opens the popover.
pub fn pill(cx: &App) -> Option<AnyElement> {
    let state = entity(cx).read(cx).state.clone();
    if !visible(&state) {
        return None;
    }
    let available = matches!(state, State::Available(_));
    let (icon_name, color) = match &state {
        State::Available(_) => ("download", 0xffffff),
        State::Error(_) => ("triangle-alert", 0xe5a050),
        State::NotFound => ("info", pal().muted),
        State::Installing(_) => ("power", pal().text),
        State::Downloading { .. } => ("download", pal().text),
        _ => ("rotate-cw", pal().text),
    };
    let glyph = icon(icon_name).size(px(11.)).text_color(rgb(color));
    let glyph = if matches!(state, State::Checking { .. } | State::Preparing(_)) {
        glyph
            .with_animation(
                "update-spin",
                Animation::new(Duration::from_secs(1)).repeat(),
                |s, t| s.with_transformation(Transformation::rotate(percentage(t))),
            )
            .into_any_element()
    } else {
        glyph.into_any_element()
    };
    Some(
        div()
            .id("update-pill")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(5.))
            .h(px(22.))
            .px(px(9.))
            .rounded_full()
            .text_size(px(11.))
            .font_weight(FontWeight::MEDIUM)
            .when(available, |d| {
                d.bg(rgb(pal().accent)).text_color(rgb(0xffffff))
            })
            .when(!available, |d| d.bg(tint(0x14)).text_color(rgb(pal().text)))
            .cursor_pointer()
            .hover(|s| s.opacity(0.85))
            .active(|s| s.opacity(0.7))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(|_, _, cx| update::toggle_popover(cx))
            .child(glyph)
            .child(div().truncate().child(pill_text(&state)))
            .into_any_element(),
    )
}

/// Where the pill is, for the popover to sit by it.
pub enum Anchor {
    /// The sidebar's foot: the popover opens upward.
    SidebarFoot,
    /// The titlebar row, this far from the window's left edge.
    Titlebar(Pixels),
}

/// The popover by the pill, with a backdrop that closes it.
pub fn popover(anchor: Anchor, top: Pixels, cx: &App) -> Option<AnyElement> {
    let u = entity(cx).read(cx);
    if !u.open || !visible(&u.state) {
        return None;
    }
    let card = div()
        .id("update-popover")
        .absolute()
        .w(px(POPOVER_W))
        .p(px(14.))
        .flex()
        .flex_col()
        .gap(px(10.))
        .rounded(px(10.))
        .bg(rgb(pal().tabbar_bg))
        .border_1()
        .border_color(rgb(pal().border))
        .shadow_lg()
        .text_size(px(12.))
        .text_color(rgb(pal().text))
        .occlude()
        .children(content(&u.state));
    let card = match anchor {
        Anchor::SidebarFoot => card.left(px(8.)).bottom(px(42.)),
        Anchor::Titlebar(x) => card.left(x).top(top),
    };
    Some(
        div()
            .absolute()
            .inset_0()
            .child(
                div()
                    .id("update-backdrop")
                    .absolute()
                    .inset_0()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| update::close_popover(cx)),
            )
            .child(card)
            .into_any_element(),
    )
}

fn title(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(13.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text.into())
}

fn muted(text: impl Into<SharedString>) -> Div {
    div().text_color(rgb(pal().muted)).child(text.into())
}

fn button(id: &'static str, label: &'static str, primary: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .px(px(10.))
        .py(px(4.))
        .rounded(px(6.))
        .text_size(px(12.))
        .when(primary, |d| {
            d.bg(rgb(pal().accent)).text_color(rgb(0xffffff))
        })
        .when(!primary, |d| d.bg(rgba(pal().hover)))
        .cursor_pointer()
        .hover(|s| s.opacity(0.85))
        .active(|s| s.opacity(0.7))
        .child(label)
}

fn buttons(children: impl IntoIterator<Item = AnyElement>) -> AnyElement {
    div()
        .flex()
        .justify_end()
        .gap(px(6.))
        .children(children)
        .into_any_element()
}

fn progress(fraction: Option<f32>) -> AnyElement {
    let bar = div().h_full().rounded_full().bg(rgb(pal().accent));
    let bar = match fraction {
        Some(f) => bar.w(relative(f.clamp(0., 1.))).into_any_element(),
        // No size to go by: a sliver that slides along.
        None => bar
            .w(relative(0.3))
            .with_animation(
                "update-progress",
                Animation::new(Duration::from_millis(1200)).repeat(),
                |d, t| d.ml(relative(0.7 * t)),
            )
            .into_any_element(),
    };
    div()
        .w_full()
        .h(px(5.))
        .rounded_full()
        .bg(rgba(pal().hover))
        .flex()
        .child(bar)
        .into_any_element()
}

fn content(state: &State) -> Vec<AnyElement> {
    let name = channel::app_name();
    match state {
        State::Idle => vec![],
        State::Checking { .. } => vec![
            title("Checking for updates…").into_any_element(),
            muted("Please wait while we check for available updates").into_any_element(),
            buttons([button("update-cancel", "Cancel", false)
                .on_click(|_, _, cx| dismiss(cx))
                .into_any_element()]),
        ],
        State::Available(r) => {
            let url = r.url.clone();
            let row = |label: &'static str, value: String| {
                div()
                    .flex()
                    .gap(px(6.))
                    .child(div().w(px(64.)).text_color(rgb(pal().muted)).child(label))
                    .child(value)
            };
            vec![
                title("Update Available").into_any_element(),
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(row("Version:", r.version.clone()))
                    .when(r.size > 0, |d| d.child(row("Size:", size(r.size))))
                    .when_some(date(&r.published), |d, when| {
                        d.child(row("Released:", when))
                    })
                    .into_any_element(),
                div()
                    .id("update-notes")
                    .text_color(rgb(pal().accent_bright))
                    .cursor_pointer()
                    .hover(|s| s.underline())
                    .child("View Release Notes")
                    .on_click(move |_, _, cx| cx.open_url(&url))
                    .into_any_element(),
                div()
                    .flex()
                    .gap(px(6.))
                    .child(button("update-skip", "Skip", false).on_click(|_, _, cx| skip(cx)))
                    .child(button("update-later", "Later", false).on_click(|_, _, cx| dismiss(cx)))
                    .child(div().flex_1())
                    .child(
                        button("update-install", "Install and Relaunch", true)
                            .on_click(|_, _, cx| install(cx)),
                    )
                    .into_any_element(),
            ]
        }
        State::Downloading { release, got } => {
            let detail = if release.size > 0 {
                format!("{} of {}", size(*got), size(release.size))
            } else {
                size(*got)
            };
            vec![
                title("Downloading Update").into_any_element(),
                progress(percent(*got, release.size).map(|p| p as f32 / 100.)),
                muted(detail).into_any_element(),
                buttons([button("update-cancel", "Cancel", false)
                    .on_click(|_, _, cx| dismiss(cx))
                    .into_any_element()]),
            ]
        }
        State::Preparing(_) => vec![
            title("Preparing Update").into_any_element(),
            progress(None),
            muted("Extracting and preparing the update").into_any_element(),
        ],
        State::Installing(_) => vec![
            title("Installing…").into_any_element(),
            progress(None),
            muted("Installing update and preparing to restart").into_any_element(),
        ],
        State::NotFound => vec![
            title("No Updates Found").into_any_element(),
            muted("You're already running the latest version.").into_any_element(),
            buttons([button("update-ok", "OK", true)
                .on_click(|_, _, cx| dismiss(cx))
                .into_any_element()]),
        ],
        State::Error(f) => {
            let releases = format!("{REPO}/releases/latest");
            let log_path = channel::state_dir().join("update.log");
            vec![
                title(f.title).text_color(rgb(0xe5a050)).into_any_element(),
                div().child(f.message.clone()).into_any_element(),
                div()
                    .flex()
                    .gap(px(6.))
                    .child(
                        button("update-log", "Log", false).on_click(move |_, _, cx| {
                            cx.open_url(&format!("file://{}", log_path.display()))
                        }),
                    )
                    .child(
                        button("update-download", "Download Latest Version", false)
                            .on_click(move |_, _, cx| cx.open_url(&releases)),
                    )
                    .child(div().flex_1())
                    .child(button("update-retry", "Retry", true).on_click(|_, _, cx| {
                        check(true, cx);
                    }))
                    .into_any_element(),
                muted(format!("{name} {}", env!("ABROMUX_VERSION")))
                    .text_size(px(10.5))
                    .into_any_element(),
            ]
        }
    }
}

/// `13.4 MB`.
fn size(bytes: u64) -> String {
    let mb = bytes as f64 / 1_000_000.;
    if mb >= 1. {
        format!("{mb:.1} MB")
    } else {
        format!("{} KB", bytes / 1000)
    }
}

/// `2026-11-01T12:13:53Z` as `Nov 1, 2026`.
fn date(iso: &str) -> Option<String> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mut parts = iso.get(..10)?.split('-');
    let (y, m, d) = (parts.next()?, parts.next()?, parts.next()?);
    let m: usize = m.parse().ok()?;
    let d: u32 = d.parse().ok()?;
    Some(format!("{} {d}, {y}", MONTHS.get(m.checked_sub(1)?)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::Release;
    // `gpui::*` brings its own `test` attribute.
    use core::prelude::v1::test;

    #[test]
    fn pill_texts() {
        let r = Release {
            version: "0.13.0".into(),
            url: String::new(),
            published: String::new(),
            asset: String::new(),
            size: 200,
            sha256: None,
        };
        assert_eq!(
            pill_text(&State::Available(r.clone())),
            "Update Available: 0.13.0"
        );
        assert_eq!(
            pill_text(&State::Downloading {
                release: r,
                got: 90
            }),
            "Downloading: 45%"
        );
        assert!(!visible(&State::Checking { user: false }));
        assert!(visible(&State::Checking { user: true }));
    }

    #[test]
    fn sizes_and_dates() {
        assert_eq!(date("2026-11-01T12:13:53Z").as_deref(), Some("Nov 1, 2026"));
        assert_eq!(size(13388873), "13.4 MB");
        assert_eq!(size(4200), "4 KB");
        assert_eq!(date("nope"), None);
    }
}
