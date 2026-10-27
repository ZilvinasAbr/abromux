//! The menu bar, grouped like cmux's. Items are the same actions the
//! keybindings and the palette use, so macOS shows each one's shortcut and
//! greys it out when nothing focused handles it (Find with no pane, Back
//! outside a browser).

use crate::{
    About, CloseTab, CloseWorkspace, CommandPalette, FlashPane, FocusAddressBar, FocusDown,
    FocusLeft, FocusRight, FocusUp, JumpToUnread, NewBrowser, NewTerminal, NewWorkspace, NextTab,
    NextWorkspace, PrevTab, PrevWorkspace, Quit, RenameTab, RenameWorkspace, SplitDown, SplitRight,
    ToggleNotifications, ToggleSidebar, ToggleZoom, WorkspaceSwitcher, browser, channel, terminal,
};
use gpui::*;

const REPO: &str = env!("CARGO_PKG_REPOSITORY");

actions!(
    menu,
    [
        Hide,
        HideOthers,
        ShowAll,
        Minimize,
        Zoom,
        OpenGuide,
        OpenShortcuts,
        OpenRepo,
        OpenIssues,
        OpenReleases
    ]
);

pub fn init(cx: &mut App) {
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &Minimize, cx| {
        if let Some(w) = cx.active_window() {
            let _ = w.update(cx, |_, window, _| window.minimize_window());
        }
    });
    cx.on_action(|_: &Zoom, cx| {
        if let Some(w) = cx.active_window() {
            let _ = w.update(cx, |_, window, _| window.zoom_window());
        }
    });
    let guide = format!("{REPO}/blob/main/docs/guide.md");
    let shortcuts = format!("{guide}#keyboard-and-mouse");
    cx.on_action(move |_: &OpenGuide, cx| cx.open_url(&guide));
    cx.on_action(move |_: &OpenShortcuts, cx| cx.open_url(&shortcuts));
    cx.on_action(|_: &OpenRepo, cx| cx.open_url(REPO));
    cx.on_action(|_: &OpenIssues, cx| cx.open_url(&format!("{REPO}/issues")));
    cx.on_action(|_: &OpenReleases, cx| cx.open_url(&format!("{REPO}/releases")));
    cx.bind_keys([
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("alt-cmd-h", HideOthers, None),
        KeyBinding::new("cmd-m", Minimize, None),
    ]);

    let app = channel::app_name();
    // The Edit items send copy:/paste:/… down the responder chain, which is
    // the only way the browser's WKWebView gets ⌘C/⌘V/⌘A/⌘X. gpui views
    // handle those keys through their own bindings before the menu sees them.
    cx.set_menus(vec![
        Menu {
            name: app.into(),
            items: vec![
                MenuItem::action(format!("About {app}"), About),
                MenuItem::separator(),
                MenuItem::os_submenu("Services", SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action(format!("Hide {app}"), Hide),
                MenuItem::action("Hide Others", HideOthers),
                MenuItem::action("Show All", ShowAll),
                MenuItem::separator(),
                MenuItem::action(format!("Quit {app}"), Quit),
            ],
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("New Workspace", NewWorkspace),
                MenuItem::action("New Terminal Tab", NewTerminal),
                MenuItem::action("New Browser Tab", NewBrowser),
                MenuItem::separator(),
                MenuItem::action("Split Right", SplitRight),
                MenuItem::action("Split Down", SplitDown),
                MenuItem::separator(),
                MenuItem::action("Rename Tab…", RenameTab),
                MenuItem::action("Rename Workspace…", RenameWorkspace),
                MenuItem::action("Restart Shell", terminal::RestartShell),
                MenuItem::separator(),
                MenuItem::action("Close Tab", CloseTab),
                MenuItem::action("Close Workspace", CloseWorkspace),
                MenuItem::separator(),
                MenuItem::action("Command Palette…", CommandPalette),
                MenuItem::action("Go to Workspace…", WorkspaceSwitcher),
            ],
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::os_action("Cut", gpui_component::input::Cut, OsAction::Cut),
                MenuItem::os_action("Copy", terminal::Copy, OsAction::Copy),
                MenuItem::os_action("Paste", terminal::Paste, OsAction::Paste),
                MenuItem::os_action(
                    "Select All",
                    gpui_component::input::SelectAll,
                    OsAction::SelectAll,
                ),
                MenuItem::separator(),
                MenuItem::action("Clear Scrollback", terminal::ClearScrollback),
                MenuItem::separator(),
                MenuItem::submenu(Menu {
                    name: "Find".into(),
                    items: vec![
                        MenuItem::action("Find…", terminal::Find),
                        MenuItem::action("Find Next", terminal::FindNext),
                        MenuItem::action("Find Previous", terminal::FindPrevious),
                        MenuItem::action("Use Selection for Find", terminal::UseSelectionForFind),
                    ],
                }),
            ],
        },
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Toggle Sidebar", ToggleSidebar),
                MenuItem::action("Toggle Pane Zoom", ToggleZoom),
                MenuItem::action("Flash Focused Pane", FlashPane),
                MenuItem::separator(),
                MenuItem::action("Increase Font Size", terminal::IncreaseFontSize),
                MenuItem::action("Decrease Font Size", terminal::DecreaseFontSize),
                MenuItem::action("Reset Font Size", terminal::ResetFontSize),
                MenuItem::separator(),
                MenuItem::action("Back", browser::Back),
                MenuItem::action("Forward", browser::Forward),
                MenuItem::action("Reload Page", browser::Reload),
                MenuItem::action("Focus Address Bar", FocusAddressBar),
                MenuItem::separator(),
                MenuItem::action("Toggle Developer Tools", browser::ToggleDevTools),
                MenuItem::action("Show JavaScript Console", browser::ShowConsole),
            ],
        },
        Menu {
            name: "Notifications".into(),
            items: vec![
                MenuItem::action("Show Notifications", ToggleNotifications),
                MenuItem::action("Jump to Latest Unread", JumpToUnread),
            ],
        },
        // gpui makes the menu named "Window" AppKit's windowsMenu, so macOS
        // appends the open windows to it.
        Menu {
            name: "Window".into(),
            items: vec![
                MenuItem::action("Minimize", Minimize),
                MenuItem::action("Zoom", Zoom),
                MenuItem::separator(),
                MenuItem::action("Next Workspace", NextWorkspace),
                MenuItem::action("Previous Workspace", PrevWorkspace),
                MenuItem::action("Next Tab", NextTab),
                MenuItem::action("Previous Tab", PrevTab),
                MenuItem::separator(),
                MenuItem::action("Focus Pane Left", FocusLeft),
                MenuItem::action("Focus Pane Right", FocusRight),
                MenuItem::action("Focus Pane Up", FocusUp),
                MenuItem::action("Focus Pane Down", FocusDown),
            ],
        },
        Menu {
            name: "Help".into(),
            items: vec![
                MenuItem::action(format!("{app} Guide"), OpenGuide),
                MenuItem::action("Keyboard Shortcuts", OpenShortcuts),
                MenuItem::separator(),
                MenuItem::action("GitHub", OpenRepo),
                MenuItem::action("Releases", OpenReleases),
                MenuItem::action("Report an Issue", OpenIssues),
            ],
        },
    ]);
}
