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
/// The row cards' background, a shade above the pane's.
const CARD_BG: u32 = 0xffffff06;

/// Make `s` the app's settings: the keymap, the menus' shortcuts and every
/// window. The config reload uses it too.
pub fn apply(s: Settings, cx: &mut App) {
    keymap::apply(&s, cx);
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
            let view = cx.new(|cx| SettingsView::new(window, cx));
            cx.new(|cx| Root::new(view, window, cx))
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
    _subscriptions: Vec<Subscription>,
}

impl SettingsView {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        window.focus(&search.focus_handle(cx));
        let _subscriptions = vec![
            cx.subscribe_in(&search, window, |this, _, ev: &InputEvent, _, cx| {
                if let InputEvent::Change = ev {
                    this.scroll.set_offset(point(px(0.), px(0.)));
                    cx.notify();
                }
            }),
            // A reload (⌘⇧,) or a change made here.
            cx.observe_global_in::<Settings>(window, |this, _, cx| {
                this.error = settings::load().err();
                cx.notify();
            }),
            // Back from editing the file by hand.
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.error = settings::load().err();
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
        ];
        Self {
            focus,
            search,
            scroll: ScrollHandle::new(),
            page: 0,
            error: settings::load().err(),
            recording: None,
            needs_modifier: false,
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
    match settings::edit(key, value) {
        Ok(s) => apply(s, cx),
        Err(e) => window.push_notification(
            Notification::error(e)
                .id::<SettingsView>()
                .title("Setting not saved")
                .autohide(false),
            cx,
        ),
    }
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
    description: &'static str,
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

/// A folder setting: the folder it comes to, and Choose… for the Finder's
/// folder picker. The reset arrow goes back to the default.
fn folder_row(title: &'static str, description: &'static str, key: Key) -> Row {
    row(
        title,
        description,
        key,
        |s| s.app.workspace_folders_directory.clone(),
        move |_, _, _, cx| {
            let shown = crate::folder::root(cx.global::<Settings>());
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
                        .text_color(rgb(MUTED))
                        .child(crate::workspace::tilde(&shown.to_string_lossy())),
                )
                .child(button("choose-folder", "Choose…").on_click(move |_, window, cx| {
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
                            let dir = crate::workspace::tilde(&dir.to_string_lossy());
                            let _ = cx.update(|window, cx| save(key, Some(json!(dir)), window, cx));
                        })
                        .detach();
                }))
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
    description: &'static str,
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
        .when(on, |d| d.justify_end().bg(rgb(ACCENT)))
        .when(!on, |d| d.bg(rgba(0xffffff26)))
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
    description: &'static str,
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

/// [− value +].
fn stepper(
    title: &'static str,
    description: &'static str,
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
                .child(svg().path(icon).size(px(12.)).text_color(rgb(TEXT)))
                .when(!enabled, |d| d.opacity(0.3))
                .when(enabled, |d| {
                    d.cursor_pointer().hover(|s| s.bg(rgba(HOVER))).on_click(
                        move |_, window, cx| {
                            // Whole steps, so 0.1 + 0.05 stays 0.15 in the file.
                            let v = ((value + delta) / step).round() * step;
                            let v = (v.clamp(min, max) * 1000.).round() / 1000.;
                            let json = serde_json::Number::from_f64(v as f64).map(Value::Number);
                            save(key, json.filter(|_| v != default), window, cx)
                        },
                    )
                })
        };
        div()
            .flex()
            .flex_none()
            .h(px(24.))
            .rounded(px(6.))
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(INPUT_BG))
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
                    .border_color(rgb(BORDER))
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
    description: &'static str,
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
            .text_color(if hex.is_some() { rgb(TEXT) } else { rgb(MUTED) })
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
    std::iter::once(rgb(ACCENT).into())
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
        .bg(rgba(HOVER))
        .border_1()
        .border_color(rgb(BORDER))
        .text_size(px(12.))
        .cursor_pointer()
        .hover(|s| s.bg(rgba(0xffffff1c)))
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

fn engine(e: BrowserEngine) -> &'static str {
    match e {
        BrowserEngine::BuiltIn => "builtIn",
        BrowserEngine::TerminalBrowser => "terminalBrowser",
    }
}

fn percent(v: f32) -> String {
    format!("{}%", (v * 100.).round())
}

fn pages(
    view: &WeakEntity<SettingsView>,
    recording: Option<&'static str>,
    needs_modifier: bool,
    cx: &App,
) -> Vec<Page> {
    vec![
        general_page(),
        sidebar_page(),
        workspace_colors_page(cx),
        panes_page(),
        shortcuts_page(view, recording, needs_modifier),
        commands_page(cx),
    ]
}

fn general_page() -> Page {
    Page {
        title: "General",
        icon: "icons/sliders-horizontal.svg",
        description: None,
        groups: vec![
            group(
                "New Workspaces",
                [
                    choice(
                        "Placement",
                        "Where ⌘N puts a new workspace in the sidebar.",
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
                    folder_row(
                        "Workspace folders",
                        "Where ⇧⌘I makes a workspace's own folder.",
                        &["app", "workspaceFoldersDirectory"],
                    ),
                ],
            ),
            group(
                "Browser",
                [choice(
                    "Browser",
                    "What new browser tabs open in. terminal-browser runs in a terminal \
                     tab; `abromux browser open` stays built-in for automation.",
                    &["browser", "engine"],
                    &[
                        ("builtIn", "Built-in"),
                        ("terminalBrowser", "terminal-browser"),
                    ],
                    |s| engine(s.browser.engine),
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
                "Automation",
                [switch(
                    "Name workspaces from agent prompts",
                    "Summarise a coding agent's first prompt into the workspace name, \
                     using that agent's own CLI. A name you give it wins.",
                    &["automation", "workspaceAutoNaming"],
                    |s| s.automation.workspace_auto_naming,
                )],
            ),
            group(
                "Config Files",
                [
                    info_row(
                        "config.json",
                        crate::workspace::tilde(&settings::path().to_string_lossy()),
                        |_, _| {
                            div()
                                .flex()
                                .gap(px(6.))
                                .child(button("open", "Open").on_click(|_, _, cx| open_config(cx)))
                                .child(button("reveal", "Show in Finder").on_click(|_, _, cx| {
                                    ensure_config();
                                    cx.reveal_path(&settings::path())
                                }))
                                .into_any_element()
                        },
                    ),
                    info_row(
                        "Ghostty config",
                        match ghostty_config() {
                            Some(p) => format!(
                                "{} · fonts, colors and terminal keys, read at launch",
                                crate::workspace::tilde(&p.to_string_lossy())
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
                    "How far a divider moves per ⌃⇧H/J/K/L press.",
                    &["panes", "resizeStep"],
                    |s| s.panes.resize_step,
                    (5., 200., 5.),
                    |v| format!("{v} px"),
                ),
                color(
                    "Divider",
                    "The line between split panes.",
                    &["panes", "dividerColor"],
                    |s| Some(s.panes.divider_color),
                ),
                color(
                    "Focused pane border",
                    "Around the focused pane when a workspace has splits.",
                    &["panes", "focusBorderColor"],
                    |s| Some(s.panes.focus_border_color),
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
    let in_ctx = |c: &str| act.bindings.iter().any(|(ctx, _)| *ctx == Some(c));
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
            "Use ⌘, ⌃ or ⌥ with the key (or a function key).".into()
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
                                d.border_color(rgb(ACCENT))
                                    .bg(rgba(ACCENT << 8 | 0x26))
                                    .text_color(rgb(ACCENT_BRIGHT))
                            } else if keys.is_empty() {
                                d.border_color(rgb(BORDER)).text_color(rgb(MUTED))
                            } else {
                                d.border_color(rgb(BORDER)).bg(rgb(INPUT_BG))
                            }
                        })
                        .hover(|s| s.border_color(rgb(MUTED)))
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
                .text_color(rgb(MUTED))
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
        .text_color(rgb(MUTED))
        .cursor_pointer()
        .hover(|s| s.bg(rgba(HOVER)).text_color(rgb(TEXT)))
        .tooltip(move |_, cx| cx.new(|_| Tooltip::new(tip)).into())
        .child(svg().path(icon).size(px(13.)).text_color(rgb(MUTED)))
}

fn render_row(row: &Row, ix: usize, window: &mut Window, cx: &mut App) -> AnyElement {
    let base = div()
        .id(("row", ix))
        .px(px(14.))
        .py(px(10.))
        .when(ix > 0, |d| d.border_t_1().border_color(rgb(BORDER)));
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
                                    .text_color(rgb(MUTED))
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
                .text_color(rgb(MUTED))
                .child(g.title.clone()),
        )
        .child(
            div()
                .rounded(px(8.))
                .border_1()
                .border_color(rgb(BORDER))
                .bg(rgba(CARD_BG))
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
                    .text_color(rgb(MUTED))
                    .child(desc),
            )
        })
        .into_any_element()
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity().downgrade();
        let pages = pages(&view, self.recording, self.needs_modifier, cx);
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

        let nav = div()
            .w(px(NAV_W))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(rgba(SIDEBAR_BG))
            .border_r_1()
            .border_color(rgb(BORDER))
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
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .rounded(px(6.))
                    .bg(rgb(INPUT_BG))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .child(
                        svg()
                            .path("icons/search.svg")
                            .size(px(13.))
                            .flex_none()
                            .text_color(rgb(MUTED)),
                    )
                    .child(
                        Input::new(&self.search)
                            .appearance(false)
                            .cleanable(true)
                            .text_size(px(13.))
                            .h_full()
                            .px_0()
                            .py_0(),
                    ),
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
                    .when(selected, |d| d.bg(rgba(0xffffff1a)))
                    .when(!selected, |d| d.hover(|s| s.bg(rgba(HOVER))))
                    .child(
                        svg()
                            .path(p.icon)
                            .size(px(14.))
                            .flex_none()
                            .text_color(if selected { rgb(TEXT) } else { rgb(MUTED) }),
                    )
                    .child(p.title)
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.select_page(ix, window, cx)),
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
            .border_color(rgb(BORDER))
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
                        .text_color(rgb(MUTED))
                        .cursor_pointer()
                        .hover(|s| s.bg(rgba(HOVER)).text_color(rgb(TEXT)))
                        // Don't start a window drag from the header.
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            svg()
                                .path("icons/undo-2.svg")
                                .size(px(12.))
                                .text_color(rgb(MUTED)),
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
                                .text_color(rgb(MUTED))
                                .child(desc),
                        )
                    })
                    .when(searching && groups.is_empty(), |d| {
                        d.child(
                            div()
                                .pt(px(24.))
                                .text_center()
                                .text_size(px(12.5))
                                .text_color(rgb(MUTED))
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
            .font_family(".AppleSystemUIFont")
            .text_size(px(13.))
            .text_color(rgb(TEXT))
            .child(nav)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .bg(rgb(PANE_BG))
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
                .child(div().text_color(rgb(MUTED)).child(e)),
        )
        .child(button("open-config", "Open config.json").on_click(|_, _, cx| open_config(cx)))
}

// MARK: Files

/// Make an empty config file if there's none, so it can be opened.
fn ensure_config() {
    let path = settings::path();
    if !path.exists() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&path, "{\n}\n");
    }
}

fn open_config(cx: &mut App) {
    ensure_config();
    cx.open_with_system(&settings::path());
}

/// The Ghostty config file abromux reads, if there is one.
fn ghostty_config() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let xdg = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home.join(".config"));
    [
        xdg.join("ghostty/config"),
        home.join("Library/Application Support/com.mitchellh.ghostty/config"),
    ]
    .into_iter()
    .find(|p| p.is_file())
}
