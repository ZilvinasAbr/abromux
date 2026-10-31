//! The Settings window (⌘,), like cmux's: a searchable list of pages on
//! the left, each page's settings on the right. It edits the same
//! `~/.config/abromux/config.json` the docs describe (`crate::settings`):
//! every change writes that one key and applies at once, a value set back
//! to its default is removed from the file, and the rest of the file is
//! left as it was. Hand edits show up here after a reload (⌘⇧,).
//!
//! The shortcut recorder takes the next key pressed. While it waits, the
//! keymap and the menus' key equivalents are switched off, so ⌘Q or ⌘W can
//! be recorded instead of quitting or closing.
//!
//! There's only ever one window; opening it again brings it to the front.

use crate::{
    channel, keymap, menu,
    settings::{self, IndicatorStyle, Placement, Settings},
    theme::*,
};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme as _, Colorize as _, Disableable as _, IconName, Root, Sizable as _,
    WindowExt as _,
    button::{Button, ButtonVariants as _},
    color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState},
    h_flex,
    menu::{DropdownMenu as _, PopupMenuItem},
    notification::Notification,
    setting::{
        AnySettingField, RenderOptions, SettingFieldElement, SettingFieldType, SettingGroup,
        SettingItem, SettingPage, Settings as SettingsPanel,
    },
    switch::Switch,
    v_flex,
};
use serde_json::{Value, json};
use std::{cell::Cell, path::PathBuf, rc::Rc};

thread_local! {
    static OPEN: Cell<Option<AnyWindowHandle>> = const { Cell::new(None) };
}

actions!(settings_window, [CloseSettings]);

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
    let bounds = Bounds::centered(None, size(px(860.), px(640.)), cx);
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
        window.focus(&focus);
        let _subscriptions = vec![
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

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity().downgrade();
        let pages = vec![
            general_page(),
            sidebar_page(),
            workspace_colors_page(cx),
            panes_page(),
            shortcuts_page(view.clone(), self.recording, self.needs_modifier),
            commands_page(cx),
        ];
        v_flex()
            .key_context("SettingsWindow")
            .track_focus(&self.focus)
            .on_action(|_: &CloseSettings, window, _| window.remove_window())
            .when(self.recording.is_some(), |d| {
                d.capture_key_down(cx.listener(Self::on_record_key))
            })
            .size_full()
            .bg(rgba(SIDEBAR_BG))
            .font_family(".AppleSystemUIFont")
            .text_color(rgb(TEXT))
            .child(
                // The titlebar: drags the window, like the main one's.
                div()
                    .id("titlebar")
                    .h(px(38.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .on_mouse_down(MouseButton::Left, |ev, window, _| {
                        crate::titlebar::drag_or_zoom(ev, window)
                    })
                    .child("Settings"),
            )
            .when_some(self.error.clone(), |d, e| d.child(error_banner(e)))
            .child(
                div().flex_1().min_h_0().child(
                    SettingsPanel::new("settings")
                        .sidebar_width(px(210.))
                        .pages(pages),
                ),
            )
            .children(Root::render_notification_layer(window, cx))
    }
}

fn error_banner(e: String) -> impl IntoElement {
    h_flex()
        .id("config-error")
        .flex_none()
        .gap_3()
        .px_4()
        .py_2()
        .bg(rgba(0xc0392b33))
        .border_b_1()
        .border_color(rgba(0xc0392b88))
        .text_size(px(12.))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("The config file has an error, so changes here can't be saved"),
                )
                .child(div().text_color(rgb(MUTED)).child(e)),
        )
        .child(
            Button::new("open-config")
                .label("Open config.json")
                .small()
                .on_click(|_, _, cx| open_config(cx)),
        )
}

// MARK: Fields

/// A setting's control. gpui-component's own fields keep their state per
/// row and don't see a value change from outside (a reload, a reset), so
/// every control here reads the settings each frame instead.
struct Field {
    ty: SettingFieldType,
    style: StyleRefinement,
    is_default: Rc<dyn Fn(&App) -> bool>,
    reset: ResetFn,
}

impl AnySettingField for Field {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn type_name(&self) -> &'static str {
        "abromux::Field"
    }
    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<Field>()
    }
    fn field_type(&self) -> &SettingFieldType {
        &self.ty
    }
    fn style(&self) -> &StyleRefinement {
        &self.style
    }
    fn is_resettable(&self, cx: &App) -> bool {
        !(self.is_default)(cx)
    }
    fn reset(&self, window: &mut Window, cx: &mut App) {
        if !(self.is_default)(cx) {
            (self.reset)(window, cx)
        }
    }
}

type Key = &'static [&'static str];
type ResetFn = Rc<dyn Fn(&mut Window, &mut App)>;

/// One row: `title`, and `control` showing `get`'s value. The page's reset
/// and the row's own reset button remove `key` from the file.
fn row<T: PartialEq + 'static>(
    title: &'static str,
    description: &'static str,
    key: Key,
    get: fn(&Settings) -> T,
    control: impl Fn(T, T, &mut Window, &mut App) -> AnyElement + 'static,
) -> SettingItem {
    let is_default = move |cx: &App| get(cx.global::<Settings>()) == get(&Settings::default());
    let reset = move |window: &mut Window, cx: &mut App| save(key, None, window, cx);
    custom_row(title, description, is_default, reset, move |window, cx| {
        let value = get(cx.global::<Settings>());
        control(value, get(&Settings::default()), window, cx)
    })
}

fn custom_row(
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    is_default: impl Fn(&App) -> bool + 'static,
    reset: impl Fn(&mut Window, &mut App) + 'static,
    control: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
) -> SettingItem {
    let is_default: Rc<dyn Fn(&App) -> bool> = Rc::new(is_default);
    let reset: ResetFn = Rc::new(reset);
    let element = {
        let (is_default, reset) = (is_default.clone(), reset.clone());
        move |_: &RenderOptions, window: &mut Window, cx: &mut App| -> AnyElement {
            h_flex()
                .gap_1()
                .when(!is_default(cx), |d| {
                    let reset = reset.clone();
                    d.child(
                        Button::new("reset")
                            .icon(IconName::Undo2)
                            .ghost()
                            .xsmall()
                            .tooltip("Reset to default")
                            .on_click(move |_, window, cx| reset(window, cx)),
                    )
                })
                .child(control(window, cx))
                .into_any_element()
        }
    };
    let element: Rc<dyn SettingFieldElement<Element = AnyElement>> = Rc::new(element);
    let field = Field {
        ty: SettingFieldType::Element { element },
        style: StyleRefinement::default(),
        is_default,
        reset,
    };
    let description: SharedString = description.into();
    let item = SettingItem::new(title, field);
    if description.is_empty() {
        item
    } else {
        item.description(description)
    }
}

fn switch(
    title: &'static str,
    description: &'static str,
    key: Key,
    get: fn(&Settings) -> bool,
) -> SettingItem {
    row(title, description, key, get, move |on, default, _, _| {
        Switch::new("switch")
            .checked(on)
            .on_click(move |on: &bool, window, cx| {
                save(key, (*on != default).then(|| json!(on)), window, cx)
            })
            .into_any_element()
    })
}

/// A dropdown of `(json value, label)` options.
fn choice(
    title: &'static str,
    description: &'static str,
    key: Key,
    options: &'static [(&'static str, &'static str)],
    get: fn(&Settings) -> &'static str,
) -> SettingItem {
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

/// − value + buttons.
#[allow(clippy::too_many_arguments)]
fn stepper(
    title: &'static str,
    description: &'static str,
    key: Key,
    get: fn(&Settings) -> f32,
    (min, max, step): (f32, f32, f32),
    format: fn(f32) -> String,
) -> SettingItem {
    row(title, description, key, get, move |value, default, _, _| {
        let set = move |delta: f32| {
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                // Whole steps, so 0.1 + 0.05 stays 0.15 in the file.
                let v = ((value + delta) / step).round() * step;
                let v = (v.clamp(min, max) * 1000.).round() / 1000.;
                let json = serde_json::Number::from_f64(v as f64).map(Value::Number);
                save(key, json.filter(|_| v != default), window, cx)
            }
        };
        h_flex()
            .gap_1()
            .child(
                Button::new("less")
                    .icon(IconName::Minus)
                    .outline()
                    .xsmall()
                    .disabled(value <= min)
                    .on_click(set(-step)),
            )
            .child(
                div()
                    .w(px(64.))
                    .text_center()
                    .text_size(px(13.))
                    .child(format(value)),
            )
            .child(
                Button::new("more")
                    .icon(IconName::Plus)
                    .outline()
                    .xsmall()
                    .disabled(value >= max)
                    .on_click(set(step)),
            )
            .into_any_element()
    })
}

/// A color, or the app's own choice when the setting has none (`None`).
fn color(
    title: &'static str,
    description: &'static str,
    key: Key,
    get: fn(&Settings) -> Option<Rgba>,
) -> SettingItem {
    row(title, description, key, get, move |value, _, window, cx| {
        let hsla = value.map(Hsla::from);
        // A new picker whenever the value changes, so it always shows what
        // the settings say (a reset, a reload) without syncing.
        let id = format!(
            "color:{}:{}",
            key.join("."),
            hsla.map(|c| c.to_hex()).unwrap_or_default()
        );
        let state = window.use_keyed_state(SharedString::from(id), cx, |window, cx| {
            let picker = cx.new(|cx| {
                let p = ColorPickerState::new(window, cx);
                match hsla {
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
        let label = match hsla {
            Some(c) => c.to_hex().to_lowercase(),
            None => "Automatic".into(),
        };
        ColorPicker::new(&picker)
            .small()
            .featured_colors(featured_colors(cx))
            .anchor(Corner::TopRight)
            .label(label)
            .into_any_element()
    })
}

struct Picker {
    picker: Entity<ColorPickerState>,
    _sub: Subscription,
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

/// A row with no setting: text on the left, `control` on the right.
fn info_row(
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    control: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
) -> SettingItem {
    custom_row(title, description, |_| true, |_, _| {}, control)
}

// MARK: Pages

fn placement(p: Placement) -> &'static str {
    match p {
        Placement::Top => "top",
        Placement::AfterCurrent => "afterCurrent",
        Placement::End => "end",
    }
}

fn general_page() -> SettingPage {
    SettingPage::new("General")
        .resettable(true)
        .group(
            SettingGroup::new()
                .title("New Workspaces")
                .item(choice(
                    "Placement",
                    "Where ⌘N puts a new workspace in the sidebar.",
                    &["app", "newWorkspacePlacement"],
                    &[
                        ("top", "Top"),
                        ("afterCurrent", "After current"),
                        ("end", "End"),
                    ],
                    |s| placement(s.app.new_workspace_placement),
                ))
                .item(choice(
                    "Placement in a group",
                    "Where a new workspace goes when made from inside a group.",
                    &["workspaceGroups", "newWorkspacePlacement"],
                    &[
                        ("afterCurrent", "After current"),
                        ("top", "Second"),
                        ("end", "Last"),
                    ],
                    |s| placement(s.workspace_groups.new_workspace_placement),
                ))
                .item(switch(
                    "Inherit working directory",
                    "Start in the focused terminal's directory instead of your home folder.",
                    &["app", "workspaceInheritWorkingDirectory"],
                    |s| s.app.workspace_inherit_working_directory,
                )),
        )
        .group(SettingGroup::new().title("Automation").item(switch(
            "Name workspaces from agent prompts",
            "Summarise a coding agent's first prompt into the workspace name, \
                 using that agent's own CLI. A name you give it wins.",
            &["automation", "workspaceAutoNaming"],
            |s| s.automation.workspace_auto_naming,
        )))
        .group(
            SettingGroup::new()
                .title("Config Files")
                .item(info_row(
                    "config.json",
                    crate::workspace::tilde(&settings::path().to_string_lossy()),
                    |_, _| {
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new("open")
                                    .label("Open")
                                    .small()
                                    .on_click(|_, _, cx| open_config(cx)),
                            )
                            .child(
                                Button::new("reveal")
                                    .label("Show in Finder")
                                    .small()
                                    .ghost()
                                    .on_click(|_, _, cx| {
                                        ensure_config();
                                        cx.reveal_path(&settings::path())
                                    }),
                            )
                            .into_any_element()
                    },
                ))
                .item(info_row(
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
                        Some(p) => Button::new("open-ghostty")
                            .label("Open")
                            .small()
                            .on_click(move |_, _, cx| cx.open_with_system(&p))
                            .into_any_element(),
                        None => div().into_any_element(),
                    },
                )),
        )
}

fn sidebar_page() -> SettingPage {
    SettingPage::new("Sidebar")
        .group(
            SettingGroup::new()
                .title("Rows")
                .item(stepper(
                    "Font size",
                    "The workspace name's size; the smaller lines scale with it.",
                    &["sidebar", "fontSize"],
                    |s| s.sidebar.font_size,
                    (8., 24., 0.5),
                    |v| format!("{v} pt"),
                ))
                .item(switch(
                    "Wrap workspace names",
                    "Let long names wrap instead of cutting them off.",
                    &["sidebar", "wrapWorkspaceTitles"],
                    |s| s.sidebar.wrap_workspace_titles,
                ))
                .item(switch(
                    "Hide all details",
                    "Only names: no description, notification, status, branch or path.",
                    &["sidebar", "hideAllDetails"],
                    |s| s.sidebar.hide_all_details,
                )),
        )
        .group(
            SettingGroup::new()
                .title("Details")
                .item(switch(
                    "Workspace description",
                    "",
                    &["sidebar", "showWorkspaceDescription"],
                    |s| s.sidebar.show_workspace_description,
                ))
                .item(switch(
                    "Latest notification",
                    "",
                    &["sidebar", "showNotificationMessage"],
                    |s| s.sidebar.show_notification_message,
                ))
                .item(switch(
                    "Git branch and directory",
                    "",
                    &["sidebar", "showBranchDirectory"],
                    |s| s.sidebar.show_branch_directory,
                ))
                .item(switch(
                    "Branch and directory on separate lines",
                    "",
                    &["sidebar", "stackBranchDirectory"],
                    |s| s.sidebar.stack_branch_directory,
                ))
                .item(switch(
                    "Last path segment only",
                    "Show …/api instead of the whole ~/ path.",
                    &["sidebar", "pathLastSegmentOnly"],
                    |s| s.sidebar.path_last_segment_only,
                ))
                .item(switch(
                    "Progress",
                    "Progress bars set with `abromux set-progress`.",
                    &["sidebar", "showProgress"],
                    |s| s.sidebar.show_progress,
                ))
                .item(switch(
                    "Log",
                    "The latest line from `abromux log`.",
                    &["sidebar", "showLog"],
                    |s| s.sidebar.show_log,
                ))
                .item(switch(
                    "Status pills",
                    "From `abromux set-status` and coding agents.",
                    &["sidebar", "showCustomMetadata"],
                    |s| s.sidebar.show_custom_metadata,
                )),
        )
        .group(
            SettingGroup::new()
                .title("Colors")
                .item(color(
                    "Selected row",
                    "",
                    &["sidebar", "selectionColor"],
                    |s| {
                        s.sidebar
                            .selection_color
                            .or(s.workspace_colors.selection_color)
                    },
                ))
                .item(color(
                    "Description text",
                    "",
                    &["sidebar", "workspaceDescriptionColor"],
                    |s| s.sidebar.workspace_description_color,
                ))
                .item(color(
                    "Tint",
                    "A color laid over the sidebar's background.",
                    &["sidebarAppearance", "tintColor"],
                    |s| s.sidebar_appearance.tint_color,
                ))
                .item(stepper(
                    "Tint opacity",
                    "",
                    &["sidebarAppearance", "tintOpacity"],
                    |s| s.sidebar_appearance.tint_opacity,
                    (0., 1., 0.05),
                    percent,
                )),
        )
}

fn percent(v: f32) -> String {
    format!("{}%", (v * 100.).round())
}

fn workspace_colors_page(cx: &App) -> SettingPage {
    let colors = cx.global::<Settings>().workspace_colors.colors.clone();
    SettingPage::new("Workspace Colors")
        .group(
            SettingGroup::new()
                .title("Indicator")
                .item(choice(
                    "Style",
                    "How a workspace's color marks its row.",
                    &["workspaceColors", "indicatorStyle"],
                    &[("leftRail", "Left rail"), ("solidFill", "Solid fill")],
                    |s| match s.workspace_colors.indicator_style {
                        IndicatorStyle::LeftRail => "leftRail",
                        IndicatorStyle::SolidFill => "solidFill",
                    },
                ))
                .item(stepper(
                    "Unselected fill opacity",
                    "With a solid fill, how strongly rows other than the selected one show their color.",
                    &["workspaceColors", "unselectedFillOpacity"],
                    |s| s.workspace_colors.unselected_fill_opacity,
                    (0., 1., 0.05),
                    percent,
                )),
        )
        .group(
            SettingGroup::new()
                .title("Palette")
                .description(
                    "The colors offered when coloring a workspace. Change them under \
                     \"workspaceColors\" › \"colors\" in config.json.",
                )
                .item(SettingItem::render(move |_, _, _| {
                    h_flex()
                        .flex_wrap()
                        .gap_2()
                        .children(colors.iter().map(|(name, c)| {
                            h_flex()
                                .gap_1p5()
                                .w(px(120.))
                                .text_size(px(12.))
                                .child(div().size(px(14.)).rounded_full().bg(rgb(*c)))
                                .child(name.clone())
                        }))
                })),
        )
}

fn panes_page() -> SettingPage {
    SettingPage::new("Panes").group(
        SettingGroup::new()
            .title("Splits")
            .item(stepper(
                "Resize step",
                "How far a divider moves per ⌃⇧H/J/K/L press.",
                &["panes", "resizeStep"],
                |s| s.panes.resize_step,
                (5., 200., 5.),
                |v| format!("{v} px"),
            ))
            .item(color(
                "Divider",
                "The line between split panes.",
                &["panes", "dividerColor"],
                |s| Some(s.panes.divider_color),
            ))
            .item(color(
                "Focused pane border",
                "Shown around the focused pane when a workspace has splits.",
                &["panes", "focusBorderColor"],
                |s| Some(s.panes.focus_border_color),
            )),
    )
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
    view: WeakEntity<SettingsView>,
    recording: Option<&'static str>,
    needs_modifier: bool,
) -> SettingPage {
    let groups = CATEGORIES.iter().map(|cat| {
        SettingGroup::new().title(*cat).items(
            keymap::ACTIONS
                .iter()
                .filter(|a| category(a) == *cat)
                .map(|act| shortcut_row(act, view.clone(), recording, needs_modifier)),
        )
    });
    SettingPage::new("Keyboard Shortcuts")
        .description(
            "Click a shortcut and press the new keys; esc cancels. Chords and \
             per-context keys can be written in config.json.",
        )
        .groups(groups)
}

fn shortcut_row(
    act: &'static keymap::Act,
    view: WeakEntity<SettingsView>,
    recording: Option<&'static str>,
    needs_modifier: bool,
) -> SettingItem {
    let name = act.name;
    let is_recording = recording == Some(name);
    custom_row(
        title(act),
        if is_recording && needs_modifier {
            "Use ⌘, ⌃ or ⌥ with the key (or a function key)."
        } else {
            ""
        },
        move |cx| !cx.global::<Settings>().shortcuts.contains_key(name),
        move |window, cx| save_shortcut(name, None, window, cx),
        move |_, cx| {
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
            let theme = cx.theme();
            let (border, muted) = (theme.border, theme.muted_foreground);
            let view = view.clone();
            h_flex()
                .gap_1()
                .child(
                    div()
                        .id("keys")
                        .min_w(px(96.))
                        .px_2()
                        .py(px(3.))
                        .rounded(px(5.))
                        .border_1()
                        .text_center()
                        .text_size(px(12.))
                        .cursor_pointer()
                        .map(|d| {
                            if is_recording {
                                d.border_color(rgb(ACCENT))
                                    .bg(rgba(ACCENT << 8 | 0x22))
                                    .text_color(rgb(ACCENT_BRIGHT))
                            } else if keys.is_empty() {
                                d.border_color(border).text_color(muted)
                            } else {
                                d.border_color(border).bg(rgba(HOVER))
                            }
                        })
                        .hover(|s| s.bg(rgba(0xffffff1c)))
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
                    Button::new("clear")
                        .icon(IconName::Close)
                        .ghost()
                        .xsmall()
                        .tooltip("Remove shortcut")
                        .disabled(keys.is_empty())
                        .on_click(move |_, window, cx| {
                            // `[]` unbinds it; `"when"` doesn't matter then.
                            save(&["shortcuts", name], Some(json!([])), window, cx)
                        }),
                )
                .into_any_element()
        },
    )
}

fn commands_page(cx: &App) -> SettingPage {
    let commands = cx.global::<Settings>().commands.clone();
    let mut group = SettingGroup::new().description(
        "Extra command palette entries from \"commands\" in config.json. A project can \
         add its own in .abromux/config.json.",
    );
    if commands.is_empty() {
        group = group.item(info_row("No commands yet", "", |_, _| {
            Button::new("open")
                .label("Open config.json")
                .small()
                .on_click(|_, _, cx| open_config(cx))
                .into_any_element()
        }));
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
        group = group.item(info_row(c.title.clone(), what, move |_, _| {
            div()
                .text_size(px(12.))
                .text_color(rgb(MUTED))
                .child(keys.clone())
                .into_any_element()
        }));
    }
    SettingPage::new("Commands").resettable(false).group(group)
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
