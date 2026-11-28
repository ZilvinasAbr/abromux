//! The Settings window (⌘,), like cmux's: pages listed on the left under a
//! search field, the page's settings on the right in cards of rows. It
//! edits the same `~/.config/abromux/config.json` the docs describe
//! (`crate::settings`): every change writes that one key and applies at
//! once, a value set back to its default is removed from the file, and the
//! rest of the file is left as it was. Hand edits show up here after a
//! reload (⌘⇧,).
//!
//! The shortcut recorder takes the next key pressed. While it waits, the
//! keymap and the menus' key equivalents are switched off, so ⌘Q or ⌘W can
//! be recorded instead of quitting or closing.
//!
//! There's only ever one window; opening it again brings it to the front.

use crate::{
    channel, keymap, menu,
    settings::{self, BrowserEngine, IndicatorStyle, Placement, Settings},
    theme::*,
};
use gpui::{prelude::*, *};
use gpui_component::{
    Colorize as _, Root, Sizable as _, WindowExt as _,
    button::Button,
    color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState},
    input::{Input, InputEvent, InputState},
    menu::{DropdownMenu as _, PopupMenuItem},
    notification::Notification,
    tooltip::Tooltip,
};
use serde_json::{Value, json};
use std::{cell::Cell, path::PathBuf, rc::Rc};

thread_local! {
    static OPEN: Cell<Option<AnyWindowHandle>> = const { Cell::new(None) };
}

actions!(settings_window, [CloseSettings]);

const NAV_W: f32 = 200.;
const TITLEBAR_H: f32 = 38.;

/// Make `s` the app's settings: the keymap, the menus' shortcuts and every
/// window. The config reload uses it too.
pub fn apply(s: Settings, cx: &mut App) {
    let panes = crate::terminal_browser_panes::set(s.browser.terminal_browser_panes);
    if let Err(e) = panes
        && let Some(window) = cx.active_window().or_else(|| cx.windows().first().copied())
    {
        let _ = window.update(cx, |_, window, cx| {
            window.push_notification(
                Notification::error(e)
                    .id::<crate::terminal_browser_panes::Failed>()
                    .title("terminal-browser not changed")
                    .autohide(false),
                cx,
            )
        });
    }
    keymap::apply(&s, cx);
    crate::theme::apply(s.app.appearance, cx);
    crate::remote::apply(&s.remote);
    crate::plugins::set_sections(&s, cx);
    cx.set_global(s);
    menu::set_menus(cx);
    cx.refresh_windows();
}

pub fn show(cx: &mut App) {
    if let Some(handle) = OPEN.get()
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }
    let bounds = Bounds::centered(None, size(px(820.), px(600.)), cx);
    let handle = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(640.), px(420.))),
            titlebar: Some(TitlebarOptions {
                title: Some(format!("{} Settings", channel::app_name()).into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(14.), px(13.))),
            }),
            window_background: WindowBackgroundAppearance::Blurred,
            is_minimizable: false,
            ..Default::default()
        },
        |window, cx| {
            crate::theme::follow(window);
            let view = cx.new(|cx| SettingsView::new(window, cx));
            let layer = cx.new(|cx| crate::alert::Layer::new(view, window, cx));
            cx.new(|cx| Root::new(layer, window, cx))
        },
    );
    if let Ok(handle) = handle {
        OPEN.set(Some(handle.into()));
        cx.activate(true);
    }
}

struct SettingsView {
    focus: FocusHandle,
    search: Entity<InputState>,
    scroll: ScrollHandle,
    /// The page shown when not searching, by index in `pages`.
    page: usize,
    /// Why the config file can't be read, if it can't: then nothing here is
    /// saved until it's fixed.
    error: Option<String>,
    /// The action whose shortcut is being recorded.
    recording: Option<&'static str>,
    /// The recorder got a key that can't be a shortcut.
    needs_modifier: bool,
    /// Plugins' text settings' fields (by path, e.g. `gitlab.host`), saved
    /// on Enter or leaving them.
    text_fields: Vec<(Path, Entity<InputState>)>,
    /// Phone Access: a Tailscale login to let in, added on Enter.
    allow_input: Entity<InputState>,
    /// Servers: a new server's name and address, added on Enter.
    server_inputs: (Entity<InputState>, Entity<InputState>),
    _subscriptions: Vec<Subscription>,
}

impl SettingsView {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        window.focus(&search.focus_handle(cx));
        let text_fields: Vec<(Path, Entity<InputState>)> = text_settings()
            .into_iter()
            .map(|(path, placeholder)| {
                let value = text_value(&path, cx.global::<Settings>());
                let input = cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(placeholder)
                        .default_value(value)
                });
                (path, input)
            })
            .collect();
        let mut _subscriptions: Vec<Subscription> = text_fields
            .iter()
            .map(|(path, input)| {
                let path = path.clone();
                cx.subscribe_in(
                    input,
                    window,
                    move |_, input, ev: &InputEvent, window, cx| {
                        if matches!(ev, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                            save_text(&path, input.read(cx).value().trim(), window, cx);
                        }
                    },
                )
            })
            .collect();
        _subscriptions.extend([
            // A reload (⌘⇧,) or hand edit shows in fields not being typed in.
            cx.observe_global_in::<Settings>(window, |this: &mut Self, window, cx| {
                for (path, input) in &this.text_fields {
                    let value = text_value(path, cx.global::<Settings>());
                    let focused = input.focus_handle(cx).is_focused(window);
                    if !focused && input.read(cx).value().trim() != value {
                        input.update(cx, |i, cx| i.set_value(value, window, cx));
                    }
                }
            }),
        ]);
        let allow_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("name@example.com, then Return"));
        _subscriptions.push(cx.subscribe_in(
            &allow_input,
            window,
            |_, input, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    let login = input.read(cx).value().trim().to_string();
                    if !login.is_empty() {
                        allow_login(&login, true, window, cx);
                        input.update(cx, |i, cx| i.set_value("", window, cx));
                    }
                }
            },
        ));
        let server_name = cx.new(|cx| InputState::new(window, cx).placeholder("Name, like vps"));
        let server_address =
            cx.new(|cx| InputState::new(window, cx).placeholder("ssh://me@host, then Return"));
        for input in [&server_name, &server_address] {
            let (name, address) = (server_name.clone(), server_address.clone());
            _subscriptions.push(cx.subscribe_in(
                input,
                window,
                move |_, _, ev: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = ev {
                        add_server(&name, &address, window, cx);
                    }
                },
            ));
        }
        // Phone access changes on its own (Tailscale answering, phones
        // coming and going), and so do servers' checks: redraw when they do.
        cx.spawn(async move |this, cx| {
            let changes = || (crate::remote::changes(), crate::servers::changes());
            let mut seen = changes();
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(700))
                    .await;
                let now = changes();
                if now != seen {
                    seen = now;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        return;
                    }
                }
            }
        })
        .detach();
        _subscriptions.extend([
            cx.subscribe_in(&search, window, |this, _, ev: &InputEvent, _, cx| {
                if let InputEvent::Change = ev {
                    this.scroll.set_offset(point(px(0.), px(0.)));
                    cx.notify();
                }
            }),
            // A reload (⌘⇧,) or a change made here.
            cx.observe_global_in::<Settings>(window, |this, _, cx| {
                this.error = settings::app_error();
                cx.notify();
            }),
            // Back from editing the file by hand.
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.error = settings::app_error();
                } else {
                    this.stop_recording(cx);
                }
                cx.notify();
            }),
            cx.on_release(|this, cx| {
                if this.recording.is_some() {
                    restore_keymap(cx);
                }
            }),
        ]);
        Self {
            focus,
            search,
            scroll: ScrollHandle::new(),
            page: 0,
            error: settings::app_error(),
            recording: None,
            needs_modifier: false,
            text_fields,
            allow_input,
            server_inputs: (server_name, server_address),
            _subscriptions,
        }
    }

    fn start_recording(&mut self, name: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        if self.recording.is_none() {
            // Nothing may run while the next key is taken.
            cx.clear_key_bindings();
            menu::set_menus(cx);
        }
        self.recording = Some(name);
        self.needs_modifier = false;
        window.focus(&self.focus);
        cx.notify();
    }

    fn stop_recording(&mut self, cx: &mut Context<Self>) {
        if self.recording.take().is_some() {
            restore_keymap(cx);
            cx.notify();
        }
    }

    fn on_record_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.recording else { return };
        cx.stop_propagation();
        let ks = &ev.keystroke;
        if ks.key == "escape" && !ks.modifiers.modified() {
            self.stop_recording(cx);
            return;
        }
        let Some(text) = keymap::record(ks) else {
            self.needs_modifier = true;
            cx.notify();
            return;
        };
        self.stop_recording(cx);
        save_shortcut(name, Some(text), window, cx);
    }

    fn select_page(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.page = ix;
        self.scroll.set_offset(point(px(0.), px(0.)));
        if !self.search.read(cx).value().is_empty() {
            self.search.update(cx, |s, cx| s.set_value("", window, cx));
        }
        cx.notify();
    }
}

/// The keymap and menus as the settings say, after recording.
fn restore_keymap(cx: &mut App) {
    let s = cx.global::<Settings>().clone();
    keymap::apply(&s, cx);
    menu::set_menus(cx);
}

/// Save one key of the config file (`None` removes it) and apply the
/// result, or say why it wasn't saved.
fn save(key: &[&str], value: Option<Value>, window: &mut Window, cx: &mut App) {
    match save_key(key, value, cx) {
        Ok(()) => {}
        Err(e) => window.push_notification(
            Notification::error(e)
                .id::<SettingsView>()
                .title("Setting not saved")
                .autohide(false),
            cx,
        ),
    }
}

/// Save one key of the config file (`None` removes it) and apply the
/// result, with init.lua's commands and settings: the Settings window's
/// and `settings.set`'s write.
pub fn save_key(key: &[&str], value: Option<Value>, cx: &mut App) -> Result<(), String> {
    let s = settings::edit(key, value)?;
    apply(settings::with_lua(s), cx);
    Ok(())
}

/// Set an action's keys to `keys` (`None` for its defaults). A shortcut
/// written with a `"when"` keeps it. Says which action, if any, the key
/// is taken from.
fn save_shortcut(name: &'static str, keys: Option<String>, window: &mut Window, cx: &mut App) {
    let s = cx.global::<Settings>();
    let Some(act) = keymap::find(name) else {
        return;
    };
    let mut taken = None;
    let mut value = keys.as_ref().map(|k| json!(k));
    if let Some(k) = &keys {
        let seq = keymap::parse_seq(k).unwrap_or_default();
        // The default key again: back to the default.
        let (defaults, _) = keymap::keys_for(&Settings::default(), act);
        if defaults == [seq.clone()] {
            value = None;
        }
        taken = keymap::ACTIONS
            .iter()
            .filter(|a| a.name != name)
            .find(|a| keymap::keys_for(s, a).0.contains(&seq))
            .map(|a| (keymap::label(&seq), title(a)));
    }
    let has_when = s.shortcuts.get(name).is_some_and(|s| s.when.is_some());
    let path: &[&str] = if has_when && value.is_some() {
        &["shortcuts", name, "keys"]
    } else {
        &["shortcuts", name]
    };
    save(path, value, window, cx);
    if let Some((label, other)) = taken {
        window.push_notification(
            Notification::warning(format!(
                "{label} now runs {} instead of {other}.",
                title(act)
            ))
            .title("Shortcut reassigned"),
            cx,
        );
    }
}

// MARK: Model

type Key = &'static [&'static str];
type ResetFn = Rc<dyn Fn(&mut Window, &mut App)>;
type IsDefaultFn = Rc<dyn Fn(&App) -> bool>;
type ControlFn = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;

#[derive(Clone)]
enum Row {
    /// A setting: its text on the left, its control on the right, and a
    /// reset arrow when it isn't the default.
    Setting {
        title: SharedString,
        description: SharedString,
        is_default: IsDefaultFn,
        reset: ResetFn,
        control: ControlFn,
    },
    /// Anything else, the card's full width. Not found by search.
    Custom(ControlFn),
}

impl Row {
    fn matches(&self, query: &str) -> bool {
        match self {
            Row::Setting {
                title, description, ..
            } => title.to_lowercase().contains(query) || description.to_lowercase().contains(query),
            Row::Custom(_) => false,
        }
    }
}

#[derive(Clone)]
struct Group {
    title: SharedString,
    description: Option<SharedString>,
    rows: Vec<Row>,
}

fn group(title: impl Into<SharedString>, rows: impl IntoIterator<Item = Row>) -> Group {
    Group {
        title: title.into(),
        description: None,
        rows: rows.into_iter().collect(),
    }
}

impl Group {
    fn description(mut self, d: impl Into<SharedString>) -> Self {
        self.description = Some(d.into());
        self
    }
}

struct Page {
    title: &'static str,
    icon: &'static str,
    description: Option<&'static str>,
    groups: Vec<Group>,
}

// MARK: Rows

/// One setting: `control` shows `get`'s value. The row's reset arrow and
/// the page's reset remove `key` from the file.
fn row<T: PartialEq + 'static>(
    title: &'static str,
    description: impl Into<SharedString>,
    key: Key,
    get: fn(&Settings) -> T,
    control: impl Fn(T, T, &mut Window, &mut App) -> AnyElement + 'static,
) -> Row {
    Row::Setting {
        title: title.into(),
        description: description.into(),
        is_default: Rc::new(move |cx| get(cx.global::<Settings>()) == get(&Settings::default())),
        reset: Rc::new(move |window, cx| save(key, None, window, cx)),
        control: Rc::new(move |window, cx| {
            let value = get(cx.global::<Settings>());
            control(value, get(&Settings::default()), window, cx)
        }),
    }
}

/// Where the workspace folders script (`abromux.workspace_folders`) puts
/// them: `$ABROMUX_WORKSPACES_DIR`, else `app.workspaceFoldersDirectory`,
/// else the channel's `~/.abromux/ws`. The script reads them the same way.
fn workspaces_dir(s: &Settings) -> std::path::PathBuf {
    std::env::var("ABROMUX_WORKSPACES_DIR")
        .ok()
        .or_else(|| s.app.workspace_folders_directory.clone())
        .filter(|d| !d.trim().is_empty())
        .map(|d| crate::paths::expand_tilde(d.trim()))
        .unwrap_or_else(crate::channel::workspaces_dir)
}

/// A folder setting: the folder it comes to, and Choose… for the Finder's
/// folder picker. The reset arrow goes back to the default.
fn folder_row(title: &'static str, description: impl Into<SharedString>, key: Key) -> Row {
    row(
        title,
        description,
        key,
        |s| s.app.workspace_folders_directory.clone(),
        move |_, _, _, cx| {
            let shown = workspaces_dir(cx.global::<Settings>());
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .min_w_0()
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(12.))
                        .text_color(rgb(pal().muted))
                        .child(crate::paths::tilde(&shown.to_string_lossy())),
                )
                .child(
                    button("choose-folder", "Choose…").on_click(move |_, window, cx| {
                        let picked = cx.prompt_for_paths(PathPromptOptions {
                            files: false,
                            directories: true,
                            multiple: false,
                            prompt: Some("Use Folder".into()),
                        });
                        window
                            .spawn(cx, async move |cx| {
                                let Ok(Ok(Some(paths))) = picked.await else {
                                    return;
                                };
                                let Some(dir) = paths.into_iter().next() else {
                                    return;
                                };
                                let dir = crate::paths::tilde(&dir.to_string_lossy());
                                let _ =
                                    cx.update(|window, cx| save(key, Some(json!(dir)), window, cx));
                            })
                            .detach();
                    }),
                )
                .into_any_element()
        },
    )
}

/// A row with no setting behind it: text, and `control` on the right.
fn info_row(
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    control: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
) -> Row {
    Row::Setting {
        title: title.into(),
        description: description.into(),
        is_default: Rc::new(|_| true),
        reset: Rc::new(|_, _| {}),
        control: Rc::new(control),
    }
}

fn switch(
    title: &'static str,
    description: impl Into<SharedString>,
    key: Key,
    get: fn(&Settings) -> bool,
) -> Row {
    row(title, description, key, get, move |on, default, _, _| {
        toggle(on)
            .on_click(move |_, window, cx| {
                let on = !on;
                save(key, (on != default).then(|| json!(on)), window, cx)
            })
            .into_any_element()
    })
}

/// An on/off switch in the app's accent.
fn toggle(on: bool) -> Stateful<Div> {
    div()
        .id("toggle")
        .flex_none()
        .w(px(30.))
        .h(px(18.))
        .p(px(2.))
        .rounded_full()
        .flex()
        .when(on, |d| d.justify_end().bg(rgb(pal().accent)))
        .when(!on, |d| d.bg(tint(0x26)))
        .cursor_pointer()
        .hover(|s| s.opacity(0.9))
        .child(
            div()
                .size(px(14.))
                .rounded_full()
                .bg(rgb(0xffffff))
                .shadow_sm(),
        )
}

/// A dropdown of `(json value, label)` options.
fn choice(
    title: &'static str,
    description: impl Into<SharedString>,
    key: Key,
    options: &'static [(&'static str, &'static str)],
    get: fn(&Settings) -> &'static str,
) -> Row {
    row(title, description, key, get, move |value, default, _, _| {
        let label = options
            .iter()
            .find(|(v, _)| *v == value)
            .map_or(value, |(_, l)| l);
        Button::new("choice")
            .label(label)
            .dropdown_caret(true)
            .outline()
            .small()
            .dropdown_menu_with_anchor(Corner::TopRight, move |menu, _, _| {
                options.iter().fold(menu, |menu, (v, l)| {
                    menu.item(PopupMenuItem::new(*l).checked(*v == value).on_click(
                        move |_, window, cx| {
                            save(key, (*v != default).then(|| json!(v)), window, cx)
                        },
                    ))
                })
            })
            .into_any_element()
    })
}

/// `notifications.needsInput`, `done` and `error`.
const ALERTS: &[(&str, &str)] = &[
    ("banner", "Banner"),
    ("unread", "Unread only"),
    ("off", "Off"),
];

/// [− value +].
fn stepper(
    title: &'static str,
    description: impl Into<SharedString>,
    key: Key,
    get: fn(&Settings) -> f32,
    (min, max, step): (f32, f32, f32),
    format: fn(f32) -> String,
) -> Row {
    row(title, description, key, get, move |value, default, _, _| {
        let button = |id: &'static str, icon: &'static str, delta: f32| {
            let enabled = if delta < 0. { value > min } else { value < max };
            div()
                .id(id)
                .w(px(26.))
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .child(svg().path(icon).size(px(12.)).text_color(rgb(pal().text)))
                .when(!enabled, |d| d.opacity(0.3))
                .when(enabled, |d| {
                    d.cursor_pointer()
                        .hover(|s| s.bg(rgba(pal().hover)))
                        .on_click(move |_, window, cx| {
                            // Whole steps, so 0.1 + 0.05 stays 0.15 in the file.
                            let v = ((value + delta) / step).round() * step;
                            let v = (v.clamp(min, max) * 1000.).round() / 1000.;
                            // Whole numbers as integers, for the settings that are.
                            let json = if v.fract() == 0. {
                                Some(json!(v as i64))
                            } else {
                                serde_json::Number::from_f64(v as f64).map(Value::Number)
                            };
                            save(key, json.filter(|_| v != default), window, cx)
                        })
                })
        };
        div()
            .flex()
            .flex_none()
            .h(px(24.))
            .rounded(px(6.))
            .border_1()
            .border_color(rgb(pal().border))
            .bg(rgb(pal().input_bg))
            .overflow_hidden()
            .child(button("less", "icons/minus.svg", -step))
            .child(
                div()
                    .w(px(60.))
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .border_l_1()
                    .border_r_1()
                    .border_color(rgb(pal().border))
                    .text_size(px(12.))
                    .child(format(value)),
            )
            .child(button("more", "icons/plus.svg", step))
            .into_any_element()
    })
}

/// A color, or the app's own choice when the setting has none (`None`).
fn color(
    title: &'static str,
    description: impl Into<SharedString>,
    key: Key,
    get: fn(&Settings) -> Option<Rgba>,
) -> Row {
    row(title, description, key, get, move |value, _, _, _| {
        ColorField {
            key,
            value: value.map(Hsla::from),
        }
        .into_any_element()
    })
}

#[derive(IntoElement)]
struct ColorField {
    key: Key,
    value: Option<Hsla>,
}

struct Picker {
    picker: Entity<ColorPickerState>,
    _sub: Subscription,
}

impl RenderOnce for ColorField {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Self { key, value } = self;
        let hex = value.map(|c| c.to_hex().to_lowercase());
        // A new picker whenever the value changes, so it always shows what
        // the settings say (a reset, a reload) without syncing.
        let id = format!(
            "color:{}:{}",
            key.join("."),
            hex.clone().unwrap_or_default()
        );
        let state = window.use_keyed_state(SharedString::from(id), cx, |window, cx| {
            let picker = cx.new(|cx| {
                let p = ColorPickerState::new(window, cx);
                match value {
                    Some(c) => p.default_value(c),
                    None => p,
                }
            });
            let sub = cx.subscribe_in(&picker, window, move |_, _, ev, window, cx| {
                let ColorPickerEvent::Change(Some(c)) = ev else {
                    return;
                };
                save(key, Some(json!(c.to_hex().to_lowercase())), window, cx);
            });
            Picker { picker, _sub: sub }
        });
        let picker = state.read(cx).picker.clone();
        div()
            .text_size(px(12.))
            .text_color(if hex.is_some() {
                rgb(pal().text)
            } else {
                rgb(pal().muted)
            })
            .child(
                ColorPicker::new(&picker)
                    .small()
                    .featured_colors(featured_colors(cx))
                    .anchor(Corner::TopRight)
                    .label(hex.unwrap_or_else(|| "Automatic".into())),
            )
    }
}

/// The app's accent and the workspace palette, first in every picker.
fn featured_colors(cx: &App) -> Vec<Hsla> {
    std::iter::once(rgb(pal().accent).into())
        .chain(
            cx.global::<Settings>()
                .workspace_colors
                .colors
                .iter()
                .map(|(_, c)| rgb(*c).into()),
        )
        .take(16)
        .collect()
}

/// A plain button, like the About window's.
fn button(id: &'static str, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .px(px(10.))
        .h(px(24.))
        .flex()
        .items_center()
        .rounded(px(6.))
        .bg(rgba(pal().hover))
        .border_1()
        .border_color(rgb(pal().border))
        .text_size(px(12.))
        .cursor_pointer()
        .hover(|s| s.bg(tint(0x1c)))
        .active(|s| s.opacity(0.7))
        .child(label)
}

// MARK: Pages

fn placement(p: Placement) -> &'static str {
    match p {
        Placement::Top => "top",
        Placement::AfterCurrent => "afterCurrent",
        Placement::End => "end",
    }
}

fn engine(e: Option<BrowserEngine>) -> &'static str {
    match e.unwrap_or_else(crate::settings::default_engine) {
        BrowserEngine::BuiltIn => "builtIn",
        BrowserEngine::TerminalBrowser => "terminalBrowser",
        BrowserEngine::System => "system",
    }
}

fn percent(v: f32) -> String {
    format!("{}%", (v * 100.).round())
}

fn pages(
    view: &WeakEntity<SettingsView>,
    recording: Option<&'static str>,
    needs_modifier: bool,
    text_fields: &[(Path, Entity<InputState>)],
    allow_input: &Entity<InputState>,
    server_inputs: &(Entity<InputState>, Entity<InputState>),
    cx: &App,
) -> Vec<Page> {
    let mut pages = vec![
        general_page(),
        sidebar_page(),
        workspace_colors_page(cx),
        panes_page(),
        shortcuts_page(view, recording, needs_modifier),
        commands_page(cx),
        scripts_page(cx),
    ];
    pages.extend(
        crate::plugins::all()
            .iter()
            .filter_map(|p| Some(plugin_page(p.id, (p.settings?)(), text_fields))),
    );
    pages.extend([phone_page(allow_input, cx), servers_page(server_inputs)]);
    pages
}

/// Add the server the Servers page's fields describe, and clear them.
fn add_server(
    name: &Entity<InputState>,
    address: &Entity<InputState>,
    window: &mut Window,
    cx: &mut App,
) {
    let n = name.read(cx).value().trim().to_string();
    let a = address.read(cx).value().trim().to_string();
    if n.is_empty() && a.is_empty() {
        return;
    }
    // A name alone: the host is the name (`vps` → ssh://vps).
    let a = if a.is_empty() { n.clone() } else { a };
    let n = if n.is_empty() {
        abromux_term::remote::Endpoint::parse(&a).map_or(a.clone(), |e| e.label())
    } else {
        n
    };
    match crate::servers::add(&n, &a) {
        Ok(s) => {
            crate::servers::check(&s.address);
            name.update(cx, |i, cx| i.set_value("", window, cx));
            address.update(cx, |i, cx| i.set_value("", window, cx));
            cx.refresh_windows();
        }
        Err(e) => window.push_notification(
            Notification::error(e)
                .id::<SettingsView>()
                .title("Server not added"),
            cx,
        ),
    }
}

/// A text field as the settings rows draw them.
fn field(input: &Entity<InputState>, width: f32) -> AnyElement {
    div()
        .w(px(width))
        .h(px(26.))
        .px(px(8.))
        .flex()
        .items_center()
        .rounded(px(6.))
        .border_1()
        .border_color(rgb(pal().border))
        .bg(rgb(pal().input_bg))
        .child(
            Input::new(input)
                .appearance(false)
                .text_size(px(12.))
                .h_full()
                .px_0()
                .py_0(),
        )
        .into_any_element()
}

/// Keep the servers on `home` (a server's name), or this app's own again:
/// off the main thread (it asks the server), then the page again.
fn set_servers_home(home: Option<String>, window: &mut Window, cx: &mut App) {
    let task = cx
        .background_executor()
        .spawn(async move { crate::servers::set_home(home.as_deref()) });
    let wh = window.window_handle();
    cx.spawn(async move |cx| {
        let r = task.await;
        let _ = wh.update(cx, |_, window, cx| {
            if let Err(e) = r {
                window.push_notification(Notification::error(e).title("Servers"), cx);
            }
            cx.refresh_windows();
        });
    })
    .detach();
}

fn servers_page(inputs: &(Entity<InputState>, Entity<InputState>)) -> Page {
    use crate::servers::Status;
    let list = crate::servers::load();
    let home = crate::servers::home();
    let rows = list.into_iter().map(|s| {
        let status = match crate::servers::status(&s.address) {
            None => String::new(),
            Some(Status::Checking) => " · checking…".into(),
            Some(Status::Up(about)) => format!(" · up: {about}"),
            Some(Status::Down(e)) => format!(" · can't reach it: {e}"),
            Some(Status::Moved(e)) => format!(" · {e}"),
        };
        let (check, remove) = (s.address.clone(), s.name.clone());
        let is_home = home.as_deref() == Some(s.address.as_str());
        let keeps = if is_home { " · keeps this list" } else { "" };
        let name = s.name.clone();
        info_row(
            s.name.clone(),
            format!("{}{status}{keeps}", s.address),
            move |_, _| {
                let (check, remove, name) = (check.clone(), remove.clone(), name.clone());
                div()
                    .flex()
                    .gap(px(6.))
                    .items_center()
                    .when(!is_home, |d| {
                        d.child(button("home-server", "Keep the List Here").on_click(
                            move |_, window, cx| set_servers_home(Some(name.clone()), window, cx),
                        ))
                    })
                    .child(button("check-server", "Check").on_click(move |_, _, cx| {
                        crate::servers::check(&check);
                        cx.refresh_windows();
                    }))
                    .child(
                        icon_button("remove-server", "icons/x.svg", "Remove").on_click(
                            move |_, window, cx| {
                                if let Err(e) = crate::servers::remove(&remove) {
                                    window.push_notification(Notification::error(e), cx);
                                }
                                cx.refresh_windows();
                            },
                        ),
                    )
                    .into_any_element()
            },
        )
    });
    let (name, address) = inputs.clone();
    let add = info_row(
        "Add a server",
        "Its name, and how ssh reaches it: ssh://user@host[:port], or a name from ~/.ssh/config.",
        move |_, _| {
            let (n, a) = (name.clone(), address.clone());
            div()
                .flex()
                .gap(px(6.))
                .items_center()
                .child(field(&name, 110.))
                .child(field(&address, 200.))
                .child(
                    button("add-server", "Add")
                        .on_click(move |_, window, cx| add_server(&n, &a, window, cx)),
                )
                .into_any_element()
        },
    );
    let path = crate::servers::path();
    Page {
        title: "Servers",
        icon: "icons/globe.svg",
        description: Some(
            "Other machines' abromux servers, such as a Linux VPS: workspaces and tabs \
             there keep running whether or not this app is open. ⌘⇧P → New Workspace on … \
             or New Tab on … opens one. The server needs abromux-server installed and ssh \
             that logs in without asking (a key in your agent, or Tailscale SSH).",
        ),
        groups: vec![
            group("Servers", rows.chain([add])),
            group(
                "Where They're Kept",
                [match home {
                    Some(h) => info_row(
                        format!("On {}", crate::servers::label(&h)),
                        "Shared: every app whose list is kept there has these servers, and a \
                         change here is a change there (`abromux servers home`). This app \
                         keeps a copy, brought up to date when it starts.",
                        |_, _| {
                            button("own-servers", "This App's Own")
                                .on_click(|_, window, cx| set_servers_home(None, window, cx))
                                .into_any_element()
                        },
                    ),
                    None => info_row(
                        crate::paths::tilde(&path.to_string_lossy()),
                        "Not in config.json, so other versions of the app sharing it don't \
                         trip on them. `abromux servers` changes it too. Keep the List Here \
                         on a server shares it with your other apps.",
                        |_, _| div().into_any_element(),
                    ),
                }],
            ),
        ],
    }
}

fn general_page() -> Page {
    Page {
        title: "General",
        icon: "icons/sliders-horizontal.svg",
        description: None,
        groups: vec![
            group(
                "Appearance",
                [choice(
                    "Appearance",
                    "Light or dark, or follow macOS's setting.",
                    &["app", "appearance"],
                    &[("system", "System"), ("light", "Light"), ("dark", "Dark")],
                    |s| s.app.appearance.as_str(),
                )],
            ),
            group(
                "New Workspaces",
                [
                    choice(
                        "Placement",
                        format!(
                            "Where {} puts a new workspace in the sidebar.",
                            crate::keymap::default_label("newWorkspace")
                        ),
                        &["app", "newWorkspacePlacement"],
                        &[
                            ("top", "Top"),
                            ("afterCurrent", "After current"),
                            ("end", "End"),
                        ],
                        |s| placement(s.app.new_workspace_placement),
                    ),
                    choice(
                        "Placement in a group",
                        "Where a new workspace goes when made from inside a group.",
                        &["workspaceGroups", "newWorkspacePlacement"],
                        &[
                            ("afterCurrent", "After current"),
                            ("top", "Second"),
                            ("end", "Last"),
                        ],
                        |s| placement(s.workspace_groups.new_workspace_placement),
                    ),
                    switch(
                        "Inherit working directory",
                        "Start in the focused terminal's directory instead of your home folder.",
                        &["app", "workspaceInheritWorkingDirectory"],
                        |s| s.app.workspace_inherit_working_directory,
                    ),
                    switch(
                        "Keep workspace open",
                        "⌘W on a workspace's last tab opens a fresh terminal instead of \
                         closing the workspace.",
                        &["app", "keepWorkspaceOpenWhenClosingLastSurface"],
                        |s| s.app.keep_workspace_open_when_closing_last_surface,
                    ),
                    switch(
                        "Reorder on notification",
                        "Move a workspace to the top of the sidebar (or of its group) \
                         when it gets a notification you aren't looking at.",
                        &["app", "reorderOnNotification"],
                        |s| s.app.reorder_on_notification,
                    ),
                    folder_row(
                        "Workspace folders",
                        "Where New Workspace Folder… (the workspace folders script in \
                         init.lua) makes a workspace's own folder.",
                        &["app", "workspaceFoldersDirectory"],
                    ),
                ],
            ),
            group(
                "Browser",
                [
                    choice(
                        "Browser",
                        "What new browser tabs open in. terminal-browser runs in a terminal \
                         tab; the default browser opens pages outside abromux, with no \
                         tab. `abromux browser open` stays built-in for automation (macOS).",
                        &["browser", "engine"],
                        if abromux_platform::MACOS {
                            &[
                                ("builtIn", "Built-in"),
                                ("terminalBrowser", "terminal-browser"),
                                ("system", "Default browser"),
                            ]
                        } else {
                            &[
                                ("terminalBrowser", "terminal-browser"),
                                ("system", "Default browser"),
                            ]
                        },
                        |s| engine(s.browser.engine),
                    ),
                    switch(
                        "terminal-browser new tabs as panes",
                        "Links that would open a terminal-browser tab open a split beside \
                         it instead. abromux edits terminal-browser's main.js for this, and \
                         again after it upgrades; it applies once all terminal-browsers have \
                         quit.",
                        &["browser", "terminalBrowserPanes"],
                        |s| s.browser.terminal_browser_panes,
                    ),
                ],
            ),
            group(
                "Editor",
                [choice(
                    "Editor",
                    "What a file opened in abromux (Finder's Open With, or a file type \
                     whose default app is abromux) opens in, in a new pane beside the \
                     focused one.",
                    &["editor", "program"],
                    &[("nvim", "Neovim"), ("tode", "tode")],
                    |s| s.editor.program.key(),
                )],
            ),
            group(
                "Updates",
                [
                    switch(
                        "Automatically check for updates",
                        "Look for a new release once a day. Nothing downloads until you \
                         choose Install and Relaunch.",
                        &["app", "automaticallyCheckForUpdates"],
                        |s| s.app.automatically_check_for_updates,
                    ),
                    info_row(
                        "Check for updates",
                        format!(
                            "You have {} {}.",
                            channel::app_name(),
                            env!("ABROMUX_VERSION")
                        ),
                        |_, _| {
                            button("check-updates", "Check Now")
                                .on_click(|_, _, cx| crate::update::check(true, cx))
                                .into_any_element()
                        },
                    ),
                ],
            ),
            group(
                "Notifications",
                [
                    choice(
                        "Needs input",
                        "A coding agent (or a program that says so) waits for you: a \
                         permission prompt or a question.",
                        &["notifications", "needsInput"],
                        ALERTS,
                        |s| s.notifications.needs_input.as_str(),
                    ),
                    choice(
                        "Done",
                        "A coding agent finished its turn, or a program is done. \
                         Unread marks the tab and workspace without a banner.",
                        &["notifications", "done"],
                        ALERTS,
                        |s| s.notifications.done.as_str(),
                    ),
                    choice(
                        "Agent finished a turn",
                        "Which of a coding agent's turns count as done. When idle skips a \
                         turn that leaves a background command, monitor or scheduled wakeup \
                         running, so polling stays quiet until the work is done.",
                        &["notifications", "agentTurnComplete"],
                        &[
                            ("whenIdle", "When idle"),
                            ("always", "Every turn"),
                            ("never", "Never"),
                        ],
                        |s| s.notifications.agent_turn_complete.as_str(),
                    ),
                    choice(
                        "Failed",
                        "A program reports it failed.",
                        &["notifications", "error"],
                        ALERTS,
                        |s| s.notifications.error.as_str(),
                    ),
                ],
            ),
            group(
                "Automation",
                [switch(
                    "Name workspaces from agent prompts",
                    "Summarise what a coding agent is asked into the workspace name, \
                     using that agent's own CLI, and keep it current. A name you give \
                     it wins. ⇧⌘I workspaces left on their made-up name always are.",
                    &["automation", "workspaceAutoNaming"],
                    |s| s.automation.workspace_auto_naming,
                )],
            ),
            group(
                "Config Files",
                [
                    info_row(
                        "config.json",
                        crate::paths::tilde(&settings::path().to_string_lossy()),
                        |_, _| {
                            div()
                                .flex()
                                .gap(px(6.))
                                .child(button("open", "Open").on_click(|_, _, cx| open_config(cx)))
                                .child(
                                    button("reveal", "Show in Finder").on_click(|_, _, cx| {
                                        cx.reveal_path(&settings::ensure_file())
                                    }),
                                )
                                .into_any_element()
                        },
                    ),
                    info_row(
                        "Ghostty config",
                        match ghostty_config() {
                            Some(p) => format!(
                                "{} · fonts, colors and terminal keys, read at launch",
                                crate::paths::tilde(&p.to_string_lossy())
                            ),
                            None => "Fonts, colors and terminal keys come from Ghostty's \
                                     config (~/.config/ghostty/config), read at launch"
                                .into(),
                        },
                        |_, _| match ghostty_config() {
                            Some(p) => button("open-ghostty", "Open")
                                .on_click(move |_, _, cx| cx.open_with_system(&p))
                                .into_any_element(),
                            None => div().into_any_element(),
                        },
                    ),
                ],
            ),
        ],
    }
}

fn sidebar_page() -> Page {
    Page {
        title: "Sidebar",
        icon: "icons/panel-left.svg",
        description: None,
        groups: vec![
            group(
                "Rows",
                [
                    stepper(
                        "Font size",
                        "The workspace name's size; the smaller lines scale with it.",
                        &["sidebar", "fontSize"],
                        |s| s.sidebar.font_size,
                        (8., 24., 0.5),
                        |v| format!("{v} pt"),
                    ),
                    switch(
                        "Wrap workspace names",
                        "Let long names wrap instead of cutting them off.",
                        &["sidebar", "wrapWorkspaceTitles"],
                        |s| s.sidebar.wrap_workspace_titles,
                    ),
                    switch(
                        "Hide all details",
                        "Only names: no description, notification, status, branch or path.",
                        &["sidebar", "hideAllDetails"],
                        |s| s.sidebar.hide_all_details,
                    ),
                ],
            ),
            group(
                "Details",
                [
                    switch(
                        "Workspace description",
                        "",
                        &["sidebar", "showWorkspaceDescription"],
                        |s| s.sidebar.show_workspace_description,
                    ),
                    switch(
                        "Tracked links",
                        format!(
                            "The selected workspace's Jira tickets, merge requests and pinned pages, under its row ({}…9).",
                            crate::keymap::default_label("link1")
                        ),
                        &["sidebar", "showLinks"],
                        |s| s.sidebar.show_links,
                    ),
                    switch(
                        "Latest notification",
                        "",
                        &["sidebar", "showNotificationMessage"],
                        |s| s.sidebar.show_notification_message,
                    ),
                    switch(
                        "Git branch and directory",
                        "",
                        &["sidebar", "showBranchDirectory"],
                        |s| s.sidebar.show_branch_directory,
                    ),
                    switch(
                        "Branch and directory on separate lines",
                        "",
                        &["sidebar", "stackBranchDirectory"],
                        |s| s.sidebar.stack_branch_directory,
                    ),
                    switch(
                        "Last path segment only",
                        "Show …/api instead of the whole ~/ path.",
                        &["sidebar", "pathLastSegmentOnly"],
                        |s| s.sidebar.path_last_segment_only,
                    ),
                    switch(
                        "Progress",
                        "Progress bars set with abromux set-progress.",
                        &["sidebar", "showProgress"],
                        |s| s.sidebar.show_progress,
                    ),
                    switch(
                        "Log",
                        "The latest line from abromux log.",
                        &["sidebar", "showLog"],
                        |s| s.sidebar.show_log,
                    ),
                    switch(
                        "Status pills",
                        "From abromux set-status and coding agents.",
                        &["sidebar", "showCustomMetadata"],
                        |s| s.sidebar.show_custom_metadata,
                    ),
                ],
            ),
            group(
                "Colors",
                [
                    color("Selected row", "", &["sidebar", "selectionColor"], |s| {
                        s.sidebar
                            .selection_color
                            .or(s.workspace_colors.selection_color)
                    }),
                    color(
                        "Description text",
                        "",
                        &["sidebar", "workspaceDescriptionColor"],
                        |s| s.sidebar.workspace_description_color,
                    ),
                    color(
                        "Tint",
                        "A color laid over the sidebar's background.",
                        &["sidebarAppearance", "tintColor"],
                        |s| s.sidebar_appearance.tint_color,
                    ),
                    stepper(
                        "Tint opacity",
                        "",
                        &["sidebarAppearance", "tintOpacity"],
                        |s| s.sidebar_appearance.tint_opacity,
                        (0., 1., 0.05),
                        percent,
                    ),
                ],
            ),
        ],
    }
}

fn workspace_colors_page(cx: &App) -> Page {
    let colors = cx.global::<Settings>().workspace_colors.colors.clone();
    Page {
        title: "Workspace Colors",
        icon: "icons/palette.svg",
        description: None,
        groups: vec![
            group(
                "Indicator",
                [
                    choice(
                        "Style",
                        "How a workspace's color marks its row.",
                        &["workspaceColors", "indicatorStyle"],
                        &[("leftRail", "Left rail"), ("solidFill", "Solid fill")],
                        |s| match s.workspace_colors.indicator_style {
                            IndicatorStyle::LeftRail => "leftRail",
                            IndicatorStyle::SolidFill => "solidFill",
                        },
                    ),
                    stepper(
                        "Unselected fill opacity",
                        "With a solid fill, how strongly the other rows show their color.",
                        &["workspaceColors", "unselectedFillOpacity"],
                        |s| s.workspace_colors.unselected_fill_opacity,
                        (0., 1., 0.05),
                        percent,
                    ),
                ],
            ),
            group(
                "Palette",
                [Row::Custom(Rc::new(move |_, _| {
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_y(px(8.))
                        .children(colors.iter().map(|(name, c)| {
                            div()
                                .w(px(120.))
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .text_size(px(12.))
                                .child(div().size(px(12.)).rounded_full().bg(rgb(*c)))
                                .child(name.clone())
                        }))
                        .into_any_element()
                }))],
            )
            .description(
                "Offered when coloring a workspace. Change them under \
                 workspaceColors › colors in config.json.",
            ),
        ],
    }
}

/// A setting's place in the file, e.g. `["gitlab", "host"]`.
type Path = Vec<String>;

/// Every plugin's text settings: their paths and placeholders.
fn text_settings() -> Vec<(Path, String)> {
    use abromux_plugin::setting::SettingKind;
    crate::plugins::all()
        .iter()
        .filter_map(|p| Some((p.id, (p.settings?)())))
        .flat_map(|(id, page)| {
            page.settings.into_iter().filter_map(move |d| match d.kind {
                SettingKind::Text { placeholder } => {
                    Some((vec![id.to_string(), d.key], placeholder.unwrap_or_default()))
                }
                _ => None,
            })
        })
        .collect()
}

/// A plugin text setting's value in `s` ("" for none).
fn text_value(path: &[String], s: &Settings) -> String {
    let [id, key] = path else {
        return String::new();
    };
    s.plugin_section(id)
        .and_then(|v| v.get(key))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// Save a text field: empty removes the key.
fn save_text(path: &[String], value: &str, window: &mut Window, cx: &mut App) {
    if value == text_value(path, cx.global::<Settings>()) {
        return;
    }
    let key: Vec<&str> = path.iter().map(String::as_str).collect();
    save(&key, (!value.is_empty()).then(|| json!(value)), window, cx);
}

/// A plugin's page: its declared settings, grouped as it says, each saved
/// in its section of the file (`"<id>": {…}`).
fn plugin_page(
    id: &'static str,
    page: abromux_plugin::setting::SettingsPage,
    fields: &[(Path, Entity<InputState>)],
) -> Page {
    let mut groups: Vec<Group> = vec![];
    for def in page.settings {
        let title = def.group.clone().unwrap_or_else(|| "Settings".into());
        let key = def.key.clone();
        let value: ValueFn = {
            let (page_def, key) = (def.clone(), key.clone());
            Rc::new(move |cx: &App| {
                cx.global::<Settings>()
                    .plugin_section(id)
                    .and_then(|s| s.get(&key))
                    .filter(|v| page_def.accepts(v))
                    .cloned()
                    .unwrap_or_else(|| page_def.default.clone())
            })
        };
        let path = vec![id.to_string(), key];
        let field = fields
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, f)| f.clone());
        let row = declared_row(def, path, value, field);
        match groups.iter_mut().find(|g| g.title.as_ref() == title) {
            Some(g) => g.rows.push(row),
            None => groups.push(group(title, [row])),
        }
    }
    Page {
        title: page.title,
        icon: page.icon,
        description: page.description,
        groups,
    }
}

// MARK: Phone Access

/// Let `login` in (or not), in `remote.allow`.
fn allow_login(login: &str, allow: bool, window: &mut Window, cx: &mut App) {
    let mut list = cx.global::<Settings>().remote.allow.clone();
    list.retain(|a| !a.eq_ignore_ascii_case(login));
    if allow {
        list.push(login.to_string());
    }
    save(
        &["remote", "allow"],
        (!list.is_empty()).then(|| json!(list)),
        window,
        cx,
    );
}

/// A URL in Tailscale's message, like the page that turns on Serve or
/// HTTPS for the tailnet.
fn link_in(text: &str) -> Option<String> {
    text.split_whitespace()
        .find(|w| w.starts_with("https://"))
        .map(|w| w.trim_end_matches(['.', ',', ')']).to_string())
}

/// The URL as a QR code for the phone's camera: dark modules on white.
fn qr_code(url: &str) -> Option<AnyElement> {
    let code = qrcode::QrCode::new(url.as_bytes()).ok()?;
    let w = code.width();
    let colors = code.to_colors();
    let cell = px(4.);
    Some(
        div()
            .flex_none()
            .p(px(10.))
            .rounded(px(8.))
            .bg(rgb(0xffffff))
            .children((0..w).map(|y| {
                div().flex().children((0..w).map(|x| {
                    let dark = colors[y * w + x] == qrcode::Color::Dark;
                    div().size(cell).when(dark, |d| d.bg(rgb(0x000000)))
                }))
            }))
            .into_any_element(),
    )
}

/// What phone access is doing: the address and its code, or what's wrong.
fn phone_status() -> AnyElement {
    let st = crate::remote::status();
    let muted = rgb(pal().muted);
    let line = |text: String| div().text_size(px(11.5)).text_color(muted).child(text);
    let problem = st["error"]
        .as_str()
        .or(st["serve_error"].as_str())
        .map(str::to_string);
    if let Some(e) = problem {
        let link = link_in(&e);
        return div()
            .flex()
            .gap(px(10.))
            .child(
                svg()
                    .path("icons/triangle-alert.svg")
                    .flex_none()
                    .size(px(16.))
                    .text_color(rgb(crate::status::WARNING)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(div().text_size(px(12.5)).child(e.clone()))
                    .child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .when(e.contains("isn't installed"), |d| {
                                d.child(button("get-tailscale", "Get Tailscale").on_click(
                                    |_, _, cx| cx.open_url("https://tailscale.com/download"),
                                ))
                            })
                            .when_some(link, |d, url| {
                                d.child(
                                    button("open-link", "Open in Browser")
                                        .on_click(move |_, _, cx| cx.open_url(&url)),
                                )
                            })
                            .child(
                                button("retry", "Try Again")
                                    .on_click(|_, _, _| crate::remote::retry()),
                            ),
                    ),
            )
            .into_any_element();
    }
    let Some(url) = st["url"].as_str().filter(|_| st["serve"].is_string()) else {
        return line("Setting up tailscale serve…".into()).into_any_element();
    };
    let url = url.to_string();
    let clients = st["clients"].as_u64().unwrap_or(0);
    let copy = url.clone();
    // Without the scheme, the address fits on a line beside the code.
    let shown = url
        .trim_start_matches("https://")
        .trim_end_matches('/')
        .to_string();
    div()
        .flex()
        .items_start()
        .gap(px(16.))
        .children(qr_code(&url))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .min_w_0()
                        .text_size(px(12.5))
                        .font_family(crate::terminal::mono_font())
                        .child(shown),
                )
                .child(
                    div()
                        .flex()
                        .child(
                            button("copy-url", "Copy Address").on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))
                            }),
                        ),
                )
                .child(line(match clients {
                    0 => "No phone connected.".into(),
                    1 => "1 phone connected.".into(),
                    n => format!("{n} phones connected."),
                }))
                .child(div().h(px(6.)))
                .child(line(
                    "On the phone, signed in to the same Tailscale account: scan the code \
                     (or open the address in Safari), then Share › Add to Home Screen, and \
                     open abromux from there."
                        .into(),
                ))
                .child(line(
                    "For alerts on the phone: Notifications in its sidebar › Turn On Alerts."
                        .into(),
                )),
        )
        .into_any_element()
}

fn phone_page(allow_input: &Entity<InputState>, cx: &App) -> Page {
    let r = &cx.global::<Settings>().remote;
    let on = r.enabled;
    let st = crate::remote::status();
    let owner = st["owner"].as_str().map(str::to_string);
    let allow = r.allow.clone();
    let input = allow_input.clone();
    let mut access = vec![switch(
        "Phone access",
        "Use this Mac's abromux from your iPhone or iPad: the workspaces, terminals, \
         Claude tabs and notifications. Over Tailscale, so only your tailnet can reach it.",
        &["remote", "enabled"],
        |s| s.remote.enabled,
    )];
    if on {
        access.push(Row::Custom(Rc::new(|_, _| phone_status())));
    }
    let who = std::iter::once(info_row(
        owner
            .clone()
            .unwrap_or_else(|| "This Mac's Tailscale user".into()),
        if owner.is_some() {
            "This Mac's Tailscale user, always let in."
        } else {
            "Always let in; Tailscale says who once it's running."
        },
        |_, _| div().into_any_element(),
    ))
    .chain(allow.into_iter().map(|login| {
        let remove = login.clone();
        info_row(login, "", move |_, _| {
            let remove = remove.clone();
            icon_button("remove-login", "icons/x.svg", "Remove")
                .on_click(move |_, window, cx| allow_login(&remove, false, window, cx))
                .into_any_element()
        })
    }))
    .chain([info_row(
        "Let someone else in",
        "Their Tailscale login. They need to be on your tailnet too.",
        move |_, _| {
            div()
                .w(px(240.))
                .h(px(26.))
                .px(px(8.))
                .flex()
                .items_center()
                .rounded(px(6.))
                .border_1()
                .border_color(rgb(pal().border))
                .bg(rgb(pal().input_bg))
                .child(
                    Input::new(&input)
                        .appearance(false)
                        .text_size(px(12.))
                        .h_full()
                        .px_0()
                        .py_0(),
                )
                .into_any_element()
        },
    )]);
    let phones = crate::remote::push_subscriptions().len();
    let cfg = crate::remote::Config::from(r);
    Page {
        title: "Phone Access",
        icon: "icons/smartphone.svg",
        description: Some(
            "Needs Tailscale on this Mac and on the phone, signed in to the same account. \
             abromux puts itself on this Mac's tailnet address with tailscale serve while \
             this is on, and takes it off when it's turned off.",
        ),
        groups: vec![
            group("Phone Access", access),
            group("Who Can Connect", who),
            group(
                "Alerts",
                [
                    stepper(
                        "Only when away for",
                        "Alerts go to the phone when no phone has abromux open and the \
                         Mac has had no keyboard or mouse use for this long.",
                        &["remote", "pushWhenIdle"],
                        |s| s.remote.push_when_idle as f32,
                        (0., 3600., 15.),
                        |v| {
                            if v == 0. {
                                "Always".into()
                            } else if v >= 60. && v % 60. == 0. {
                                format!("{} min", v / 60.)
                            } else {
                                format!("{v} s")
                            }
                        },
                    ),
                    switch(
                        "Show the message",
                        "The alert has what the agent said, not just which one it is.",
                        &["remote", "pushPreview"],
                        |s| s.remote.push_preview,
                    ),
                    info_row(
                        match phones {
                            0 => "No phone gets alerts".to_string(),
                            1 => "1 phone gets alerts".to_string(),
                            n => format!("{n} phones get alerts"),
                        },
                        "Sent through Apple's push service, encrypted for each phone.",
                        move |_, _| {
                            div()
                                .flex()
                                .gap(px(6.))
                                .when(phones > 0, |d| {
                                    d.child(button("send-test", "Send Test").on_click(|_, _, _| {
                                        std::thread::spawn(|| {
                                            crate::remote::push_all(&json!({
                                                "title": channel::app_name(),
                                                "body": "Notifications from this Mac work.",
                                            }))
                                        });
                                    }))
                                    .child(
                                        button("forget-phones", "Forget All").on_click(
                                            |_, _, cx| {
                                                let _ = crate::remote::push_clear();
                                                cx.refresh_windows();
                                            },
                                        ),
                                    )
                                })
                                .into_any_element()
                        },
                    ),
                ],
            ),
            group(
                "This Mac",
                [
                    switch(
                        "Stay awake while agents work",
                        "While phone access is on and an agent is working, the Mac doesn't \
                         go to sleep on its own (the display still does).",
                        &["remote", "keepAwake"],
                        |s| s.remote.keep_awake,
                    ),
                    switch(
                        "Phones can do everything",
                        "Also browser automation (your signed-in browser) and files. Off: \
                         workspaces, terminals, Claude tabs and notifications.",
                        &["remote", "allMethods"],
                        |s| s.remote.all_methods,
                    ),
                    info_row(
                        "Ports",
                        "\"port\" and \"httpsPort\" under \"remote\" in the config file.",
                        move |_, _| {
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(pal().muted))
                                .child(format!(
                                    "127.0.0.1:{} → tailnet :{}",
                                    cfg.port, cfg.https_port
                                ))
                                .into_any_element()
                        },
                    ),
                ],
            ),
        ],
    }
}

fn panes_page() -> Page {
    Page {
        title: "Panes",
        icon: "icons/split-right.svg",
        description: None,
        groups: vec![group(
            "Splits",
            [
                stepper(
                    "Resize step",
                    format!(
                        "How far a divider moves per {}/J/K/L press.",
                        crate::keymap::default_label("resizeLeft")
                    ),
                    &["panes", "resizeStep"],
                    |s| s.panes.resize_step,
                    (5., 200., 5.),
                    |v| format!("{v} px"),
                ),
                color(
                    "Divider",
                    "The line between split panes.",
                    &["panes", "dividerColor"],
                    |s| s.panes.divider_color,
                ),
                color(
                    "Focused pane border",
                    "Around the focused pane when a workspace has splits.",
                    &["panes", "focusBorderColor"],
                    |s| Some(s.panes.focus_border_color),
                ),
                choice(
                    "Open links in",
                    "Where a web page or Markdown file ⌘-clicked in a terminal opens: \
                     a new tab in the terminal's own pane, or a pane to the right of it.",
                    &["panes", "openLinksIn"],
                    &[("samePane", "Same pane"), ("newPane", "New pane beside")],
                    |s| s.panes.open_links_in.key(),
                ),
            ],
        )],
    }
}

/// An action's name as the palette shows it, or one made up for the
/// actions it doesn't list.
fn title(act: &keymap::Act) -> String {
    if let Some(t) = act.title {
        return t.trim_end_matches('…').to_string();
    }
    let n = |p: &str| act.name.strip_prefix(p).map(str::to_string);
    match act.name {
        "commandPalette" => "Command Palette".into(),
        "workspaceSwitcher" => "Go to Workspace".into(),
        _ => n("workspace")
            .map(|i| format!("Select Workspace {i}"))
            .or_else(|| n("tab").map(|i| format!("Select Tab {i}")))
            .unwrap_or_else(|| act.name.to_string()),
    }
}

/// The Shortcuts page's group for an action.
fn category(act: &keymap::Act) -> &'static str {
    let in_ctx = |c: &str| act.defaults().iter().any(|(ctx, _)| *ctx == Some(c));
    let name = act.name.to_ascii_lowercase();
    if in_ctx("Terminal") {
        "Terminal"
    } else if in_ctx("Browser") || name.contains("addressbar") {
        "Browser"
    } else if name.contains("workspace") || name.contains("session") {
        "Workspaces"
    } else if ["tab", "split", "focus", "swap", "resize", "pane", "zoom"]
        .iter()
        .any(|w| name.contains(w))
    {
        "Tabs & Panes"
    } else {
        "App"
    }
}

const CATEGORIES: [&str; 5] = ["App", "Workspaces", "Tabs & Panes", "Terminal", "Browser"];

fn shortcuts_page(
    view: &WeakEntity<SettingsView>,
    recording: Option<&'static str>,
    needs_modifier: bool,
) -> Page {
    Page {
        title: "Keyboard Shortcuts",
        icon: "icons/keyboard.svg",
        description: Some(
            "Click a shortcut and press the new keys; esc cancels. Chords and \
             per-context keys can be written in config.json.",
        ),
        groups: CATEGORIES
            .iter()
            .map(|cat| {
                group(
                    *cat,
                    keymap::ACTIONS
                        .iter()
                        .filter(|a| category(a) == *cat)
                        .map(|act| shortcut_row(act, view.clone(), recording, needs_modifier)),
                )
            })
            .collect(),
    }
}

fn shortcut_row(
    act: &'static keymap::Act,
    view: WeakEntity<SettingsView>,
    recording: Option<&'static str>,
    needs_modifier: bool,
) -> Row {
    let name = act.name;
    let is_recording = recording == Some(name);
    Row::Setting {
        title: title(act).into(),
        description: if is_recording && needs_modifier {
            if abromux_platform::MACOS {
                "Use ⌘, ⌃ or ⌥ with the key (or a function key).".into()
            } else {
                "Use Ctrl or Alt with the key (or a function key).".into()
            }
        } else {
            "".into()
        },
        is_default: Rc::new(move |cx| !cx.global::<Settings>().shortcuts.contains_key(name)),
        reset: Rc::new(move |window, cx| save_shortcut(name, None, window, cx)),
        control: Rc::new(move |_, cx| {
            let (keys, _) = keymap::keys_for(cx.global::<Settings>(), act);
            let label = if is_recording {
                "Press keys…".to_string()
            } else if keys.is_empty() {
                "None".to_string()
            } else {
                keys.iter()
                    .map(|k| keymap::label(k))
                    .collect::<Vec<_>>()
                    .join("  ")
            };
            let view = view.clone();
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .child(
                    div()
                        .id("keys")
                        .min_w(px(88.))
                        .h(px(24.))
                        .px(px(8.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.))
                        .border_1()
                        .text_size(px(12.))
                        .cursor_pointer()
                        .map(|d| {
                            if is_recording {
                                d.border_color(rgb(pal().accent))
                                    .bg(rgba(pal().accent << 8 | 0x26))
                                    .text_color(rgb(pal().accent_bright))
                            } else if keys.is_empty() {
                                d.border_color(rgb(pal().border))
                                    .text_color(rgb(pal().muted))
                            } else {
                                d.border_color(rgb(pal().border)).bg(rgb(pal().input_bg))
                            }
                        })
                        .hover(|s| s.border_color(rgb(pal().muted)))
                        .child(label)
                        .on_click(move |_, window, cx| {
                            view.update(cx, |v, cx| {
                                if is_recording {
                                    v.stop_recording(cx)
                                } else {
                                    v.start_recording(name, window, cx)
                                }
                            })
                            .ok();
                        }),
                )
                .child(
                    icon_button("clear", "icons/x.svg", "Remove shortcut")
                        .when(keys.is_empty(), |d| d.invisible())
                        .on_click(move |_, window, cx| {
                            // `[]` unbinds it; `"when"` doesn't matter then.
                            save(&["shortcuts", name], Some(json!([])), window, cx)
                        }),
                )
                .into_any_element()
        }),
    }
}

fn commands_page(cx: &App) -> Page {
    let commands = cx.global::<Settings>().commands.clone();
    let mut rows = vec![];
    if commands.is_empty() {
        rows.push(info_row(
            "No commands yet",
            "Add them under \"commands\" in config.json.",
            |_, _| {
                button("open", "Open config.json")
                    .on_click(|_, _, cx| open_config(cx))
                    .into_any_element()
            },
        ));
    }
    for c in commands {
        let keys = c
            .shortcut
            .as_ref()
            .map(|s| {
                s.keys
                    .iter()
                    .map(|k| keymap::label(k))
                    .collect::<Vec<_>>()
                    .join("  ")
            })
            .unwrap_or_default();
        let what = c.describe().replace('\n', " ");
        rows.push(info_row(c.title.clone(), what, move |_, _| {
            div()
                .text_size(px(12.))
                .text_color(rgb(pal().muted))
                .child(keys.clone())
                .into_any_element()
        }));
    }
    Page {
        title: "Commands",
        icon: "icons/terminal.svg",
        description: Some(
            "Extra command palette entries from \"commands\" in config.json. A project \
             can add its own in .abromux/config.json.",
        ),
        groups: vec![group("Commands", rows)],
    }
}

/// The settings init.lua declares (`abromux.setting`), grouped as it says.
fn scripts_page(cx: &App) -> Page {
    let s = cx.global::<Settings>();
    let mut groups: Vec<Group> = vec![];
    for def in s.script_settings.clone() {
        let title = def.group.clone().unwrap_or_else(|| "Settings".into());
        let key = def.key.clone();
        let value: ValueFn = Rc::new(move |cx: &App| {
            cx.global::<Settings>()
                .script_value(&key)
                .unwrap_or(Value::Null)
        });
        let path = vec!["scripts".to_string(), def.key.clone()];
        let row = declared_row(def, path, value, None);
        match groups.iter_mut().find(|g| g.title.as_ref() == title) {
            Some(g) => g.rows.push(row),
            None => groups.push(group(title, [row])),
        }
    }
    if groups.is_empty() {
        groups.push(group(
            "Settings",
            [info_row(
                "No script settings yet",
                "Declare them in init.lua with abromux.setting{...}.",
                |_, _| {
                    button("open-init", "Open init.lua")
                        .on_click(|_, _, cx| cx.open_with_system(&crate::lua::ensure_init_file()))
                        .into_any_element()
                },
            )],
        ));
    }
    Page {
        title: "Scripts",
        icon: "icons/pencil.svg",
        description: Some("Settings your init.lua scripts declare."),
        groups,
    }
}

/// A declared setting's current value (its default when the file has none).
type ValueFn = Rc<dyn Fn(&App) -> Value>;

/// The row of a declared setting (a script's or a plugin's), kept at
/// `path` in the file; a text one's field is made with the view (`field`).
fn declared_row(
    def: abromux_plugin::setting::SettingDef,
    path: Path,
    value: ValueFn,
    field: Option<Entity<InputState>>,
) -> Row {
    use abromux_plugin::setting::SettingKind;
    let default = def.default.clone();
    let save_at = {
        let path = path.clone();
        move |v: Option<Value>, window: &mut Window, cx: &mut App| {
            let key: Vec<&str> = path.iter().map(String::as_str).collect();
            save(&key, v, window, cx)
        }
    };
    let set = {
        let default = default.clone();
        let save_at = save_at.clone();
        Rc::new(move |v: Value, window: &mut Window, cx: &mut App| {
            save_at((v != default).then_some(v), window, cx)
        })
    };
    let control: ControlFn = match def.kind {
        SettingKind::Boolean => {
            let value = value.clone();
            Rc::new(move |_, cx| {
                let on = value(cx).as_bool().unwrap_or(false);
                let set = set.clone();
                toggle(on)
                    .on_click(move |_, window, cx| set(json!(!on), window, cx))
                    .into_any_element()
            })
        }
        SettingKind::Choice(options) => {
            let value = value.clone();
            let options = Rc::new(options);
            Rc::new(move |_, cx| {
                let current = value(cx).as_str().unwrap_or_default().to_string();
                let label = options
                    .iter()
                    .find(|(v, _)| *v == current)
                    .map_or(current.clone(), |(_, l)| l.clone());
                let options = options.clone();
                let set = set.clone();
                Button::new("choice")
                    .label(label)
                    .dropdown_caret(true)
                    .outline()
                    .small()
                    .dropdown_menu_with_anchor(Corner::TopRight, move |menu, _, _| {
                        options.iter().fold(menu, |menu, (v, l)| {
                            let set = set.clone();
                            let v = v.clone();
                            menu.item(
                                PopupMenuItem::new(l.clone())
                                    .checked(v == current)
                                    .on_click(move |_, window, cx| set(json!(v), window, cx)),
                            )
                        })
                    })
                    .into_any_element()
            })
        }
        SettingKind::Text { .. } => {
            let field = field.clone();
            Rc::new(move |_, _| match &field {
                Some(input) => div()
                    .w(px(260.))
                    .h(px(26.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(rgb(pal().border))
                    .bg(rgb(pal().input_bg))
                    .child(
                        Input::new(input)
                            .appearance(false)
                            .text_size(px(12.))
                            .h_full()
                            .px_0()
                            .py_0(),
                    )
                    .into_any_element(),
                None => div().into_any_element(),
            })
        }
        SettingKind::Number {
            min,
            max,
            step,
            unit,
        } => {
            let value = value.clone();
            Rc::new(move |_, cx| {
                let n = value(cx).as_f64().unwrap_or(min);
                let button = |id: &'static str, icon: &'static str, delta: f64| {
                    let enabled = if delta < 0. { n > min } else { n < max };
                    let set = set.clone();
                    div()
                        .id(id)
                        .w(px(26.))
                        .h_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(svg().path(icon).size(px(12.)).text_color(rgb(pal().text)))
                        .when(!enabled, |d| d.opacity(0.3))
                        .when(enabled, |d| {
                            d.cursor_pointer()
                                .hover(|s| s.bg(rgba(pal().hover)))
                                .on_click(move |_, window, cx| {
                                    let v = ((n + delta) / step).round() * step;
                                    let v = (v.clamp(min, max) * 1000.).round() / 1000.;
                                    let v = if v.fract() == 0. {
                                        json!(v as i64)
                                    } else {
                                        json!(v)
                                    };
                                    set(v, window, cx)
                                })
                        })
                };
                div()
                    .flex()
                    .flex_none()
                    .h(px(24.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(rgb(pal().border))
                    .bg(rgb(pal().input_bg))
                    .overflow_hidden()
                    .child(button("less", "icons/minus.svg", -step))
                    .child(
                        div()
                            .w(px(60.))
                            .h_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .border_l_1()
                            .border_r_1()
                            .border_color(rgb(pal().border))
                            .text_size(px(12.))
                            .child(match &unit {
                                Some(u) => format!("{n} {u}"),
                                None => format!("{n}"),
                            }),
                    )
                    .child(button("more", "icons/plus.svg", step))
                    .into_any_element()
            })
        }
    };
    Row::Setting {
        title: def.title.into(),
        description: def.description.unwrap_or_default().into(),
        is_default: Rc::new(move |cx| value(cx) == default),
        reset: Rc::new(move |window, cx| save_at(None, window, cx)),
        control,
    }
}

// MARK: Rendering

/// A small icon that does something, with a tooltip.
fn icon_button(id: &'static str, icon: &'static str, tip: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .size(px(22.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .text_color(rgb(pal().muted))
        .cursor_pointer()
        .hover(|s| s.bg(rgba(pal().hover)).text_color(rgb(pal().text)))
        .tooltip(move |_, cx| cx.new(|_| Tooltip::new(tip)).into())
        .child(svg().path(icon).size(px(13.)).text_color(rgb(pal().muted)))
}

fn render_row(row: &Row, ix: usize, window: &mut Window, cx: &mut App) -> AnyElement {
    let base = div()
        .id(("row", ix))
        .px(px(14.))
        .py(px(10.))
        .when(ix > 0, |d| d.border_t_1().border_color(rgb(pal().border)));
    match row {
        Row::Custom(control) => base.child(control(window, cx)).into_any_element(),
        Row::Setting {
            title,
            description,
            is_default,
            reset,
            control,
        } => {
            let reset = reset.clone();
            base.min_h(px(44.))
                .flex()
                .items_center()
                .gap(px(16.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(div().text_size(px(13.)).child(title.clone()))
                        .when(!description.is_empty(), |d| {
                            d.child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(rgb(pal().muted))
                                    .child(description.clone()),
                            )
                        }),
                )
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .when(!is_default(cx), |d| {
                            d.child(
                                icon_button("reset", "icons/undo-2.svg", "Reset to default")
                                    .on_click(move |_, window, cx| reset(window, cx)),
                            )
                        })
                        .child(control(window, cx)),
                )
                .into_any_element()
        }
    }
}

fn render_group(g: &Group, ix: usize, window: &mut Window, cx: &mut App) -> AnyElement {
    div()
        .id(("group", ix))
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(
            div()
                .px(px(4.))
                .text_size(px(11.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(pal().muted))
                .child(g.title.clone()),
        )
        .child(
            div()
                .rounded(px(8.))
                .border_1()
                .border_color(rgb(pal().border))
                .bg(rgba(pal().card_bg))
                .overflow_hidden()
                .children(
                    g.rows
                        .iter()
                        .enumerate()
                        .map(|(i, r)| render_row(r, i, window, cx)),
                ),
        )
        .when_some(g.description.clone(), |d, desc| {
            d.child(
                div()
                    .px(px(4.))
                    .text_size(px(11.5))
                    .text_color(rgb(pal().muted))
                    .child(desc),
            )
        })
        .into_any_element()
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity().downgrade();
        let pages = pages(
            &view,
            self.recording,
            self.needs_modifier,
            &self.text_fields,
            &self.allow_input,
            &self.server_inputs,
            cx,
        );
        self.page = self.page.min(pages.len() - 1);
        let query = self.search.read(cx).value().trim().to_lowercase();
        let searching = !query.is_empty();

        // What the right side shows: the page, or every match.
        let (heading, description, groups): (SharedString, Option<&str>, Vec<Group>) = if searching
        {
            let groups = pages
                .iter()
                .flat_map(|p| {
                    let query = &query;
                    let page_hit = p.title.to_lowercase().contains(query);
                    p.groups.iter().filter_map(move |g| {
                        let all = page_hit || g.title.to_lowercase().contains(query);
                        let rows: Vec<Row> = g
                            .rows
                            .iter()
                            .filter(|r| {
                                matches!(r, Row::Setting { .. }) && (all || r.matches(query))
                            })
                            .cloned()
                            .collect();
                        (!rows.is_empty()).then(|| Group {
                            title: format!("{} › {}", p.title, g.title).into(),
                            description: None,
                            rows,
                        })
                    })
                })
                .collect();
            ("Search".into(), None, groups)
        } else {
            let p = &pages[self.page];
            (p.title.into(), p.description, p.groups.clone())
        };
        let changed: Vec<ResetFn> = groups
            .iter()
            .flat_map(|g| &g.rows)
            .filter_map(|r| match r {
                Row::Setting {
                    is_default, reset, ..
                } if !is_default(cx) => Some(reset.clone()),
                _ => None,
            })
            .collect();

        let nav =
            div()
                .w(px(NAV_W))
                .flex_none()
                .h_full()
                .flex()
                .flex_col()
                .bg(rgba(pal().sidebar_bg))
                .border_r_1()
                .border_color(rgb(pal().border))
                .child(
                    div()
                        .id("nav-titlebar")
                        .h(px(TITLEBAR_H))
                        .flex_none()
                        .on_mouse_down(MouseButton::Left, |ev, window, _| {
                            crate::titlebar::drag_or_zoom(ev, window)
                        }),
                )
                .child(
                    div()
                        .mx(px(10.))
                        .mb(px(8.))
                        .h(px(28.))
                        .pl(px(8.))
                        .pr(px(3.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .rounded(px(6.))
                        .bg(rgb(pal().input_bg))
                        .border_1()
                        .border_color(rgb(pal().border))
                        .child(
                            svg()
                                .path("icons/search.svg")
                                .size(px(13.))
                                .flex_none()
                                .text_color(rgb(pal().muted)),
                        )
                        .child(
                            // Our own clear button below, not `cleanable`: its
                            // suffix brings 12px of right padding we can't undo,
                            // which leaves the ⓧ well short of the field's edge.
                            Input::new(&self.search)
                                .appearance(false)
                                .text_size(px(13.))
                                .flex_1()
                                .min_w_0()
                                .h_full()
                                .px_0()
                                .py_0(),
                        )
                        .when(searching, |d| {
                            d.child(
                                icon_button("clear-search", "icons/circle-x.svg", "Clear search")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.search.update(cx, |s, cx| s.set_value("", window, cx));
                                        window.focus(&this.search.focus_handle(cx));
                                        cx.notify();
                                    })),
                            )
                        }),
                )
                .children(pages.iter().enumerate().map(|(ix, p)| {
                    let selected = !searching && ix == self.page;
                    div()
                        .id(("nav", ix))
                        .mx(px(6.))
                        .px(px(10.))
                        .h(px(28.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .rounded(px(6.))
                        .text_size(px(13.))
                        .cursor_pointer()
                        .when(selected, |d| d.bg(tint(0x1a)))
                        .when(!selected, |d| d.hover(|s| s.bg(rgba(pal().hover))))
                        .child(svg().path(p.icon).size(px(14.)).flex_none().text_color(
                            if selected {
                                rgb(pal().text)
                            } else {
                                rgb(pal().muted)
                            },
                        ))
                        .child(p.title)
                        .on_click(
                            cx.listener(move |this, _, window, cx| {
                                this.select_page(ix, window, cx)
                            }),
                        )
                }));

        let header = div()
            .id("titlebar")
            .h(px(TITLEBAR_H))
            .flex_none()
            .px(px(24.))
            .flex()
            .items_center()
            .justify_between()
            .border_b_1()
            .border_color(rgb(pal().border))
            .on_mouse_down(MouseButton::Left, |ev, window, _| {
                crate::titlebar::drag_or_zoom(ev, window)
            })
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(heading),
            )
            .when(!searching && !changed.is_empty(), |d| {
                d.child(
                    div()
                        .id("reset-page")
                        .px(px(8.))
                        .h(px(22.))
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .rounded(px(5.))
                        .text_size(px(12.))
                        .text_color(rgb(pal().muted))
                        .cursor_pointer()
                        .hover(|s| s.bg(rgba(pal().hover)).text_color(rgb(pal().text)))
                        // Don't start a window drag from the header.
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            svg()
                                .path("icons/undo-2.svg")
                                .size(px(12.))
                                .text_color(rgb(pal().muted)),
                        )
                        .child("Reset Page")
                        .on_click(move |_, window, cx| {
                            for reset in &changed {
                                reset(window, cx);
                            }
                        }),
                )
            });

        let body = div()
            .id("content")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(
                div()
                    .w_full()
                    .max_w(px(680.))
                    .mx_auto()
                    .px(px(24.))
                    .pt(px(16.))
                    .pb(px(32.))
                    .flex()
                    .flex_col()
                    .gap(px(20.))
                    .when_some(description, |d, desc| {
                        d.child(
                            div()
                                .px(px(4.))
                                .text_size(px(12.))
                                .text_color(rgb(pal().muted))
                                .child(desc),
                        )
                    })
                    .when(searching && groups.is_empty(), |d| {
                        d.child(
                            div()
                                .pt(px(24.))
                                .text_center()
                                .text_size(px(12.5))
                                .text_color(rgb(pal().muted))
                                .child("No settings match your search."),
                        )
                    })
                    .children(
                        groups
                            .iter()
                            .enumerate()
                            .map(|(i, g)| render_group(g, i, window, cx)),
                    ),
            );

        div()
            .key_context("SettingsWindow")
            .track_focus(&self.focus)
            .on_action(|_: &CloseSettings, window, _| window.remove_window())
            .when(self.recording.is_some(), |d| {
                d.capture_key_down(cx.listener(Self::on_record_key))
            })
            .size_full()
            .flex()
            .font_family(abromux_platform::fonts::ui())
            .text_size(px(13.))
            .text_color(rgb(pal().text))
            .child(nav)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .bg(rgb(pal().pane_bg))
                    .child(header)
                    .when_some(self.error.clone(), |d, e| d.child(error_banner(e)))
                    .child(body),
            )
            .children(Root::render_notification_layer(window, cx))
    }
}

fn error_banner(e: String) -> impl IntoElement {
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(24.))
        .py(px(8.))
        .bg(rgba(0xc0392b2e))
        .border_b_1()
        .border_color(rgba(0xc0392b80))
        .text_size(px(12.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("The config file has an error, so changes here can't be saved."),
                )
                .child(div().text_color(rgb(pal().muted)).child(e)),
        )
        .child(button("open-config", "Open config.json").on_click(|_, _, cx| open_config(cx)))
}

// MARK: Files

pub fn open_config(cx: &mut App) {
    cx.open_with_system(&settings::ensure_file());
}

/// The Ghostty config file whose settings win: the last one abromux reads.
fn ghostty_config() -> Option<PathBuf> {
    crate::paths::ghostty_configs()
        .into_iter()
        .rev()
        .find(|p| p.is_file())
}
