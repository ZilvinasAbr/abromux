//! The menu bar, grouped like cmux's. Items are the same actions the
//! keybindings and the palette use, so macOS shows each one's shortcut and
//! greys it out when nothing focused handles it (Find with no pane, Back
//! outside a browser).

use crate::{
    About, CloseTab, CloseWorkspace, CommandPalette, EditDescription, FlashPane, FocusAddressBar,
    FocusDown, FocusLeft, FocusRight, FocusUp, GroupByFolder, ImportCmuxSession, JumpToUnread,
    LastPane, MarkOldestUnreadAndJumpNext, MoveWorkspaceDown, MoveWorkspaceUp, NewBrowser,
    NewClaudeChat, NewGroup, NewTerminal, NewWorkspace, NextTab, NextWorkspace, OpenConfig,
    OpenInitLua, OpenSettings, PrevTab, PrevWorkspace, Quit, ReloadConfig, RenameTab,
    RenameWorkspace, ReopenSession, SplitDown, SplitRight, ToggleClaudeView, ToggleGroup,
    ToggleNotifications, ToggleSidebar, ToggleUnread, ToggleWorkspaceUnread, ToggleZoom, Ungroup,
    WorkspaceSwitcher, browser, channel, terminal,
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
    cx.on_action(|a: &OpenGuide, cx| help(a, cx));
    cx.on_action(|a: &OpenShortcuts, cx| help(a, cx));
    cx.on_action(|a: &OpenRepo, cx| help(a, cx));
    cx.on_action(|a: &OpenIssues, cx| help(a, cx));
    cx.on_action(|a: &OpenReleases, cx| help(a, cx));
    // macOS's; Linux has no hiding, and Super is the compositor's.
    if abromux_platform::MACOS {
        cx.bind_keys([
            KeyBinding::new("cmd-h", Hide, None),
            KeyBinding::new("alt-cmd-h", HideOthers, None),
            KeyBinding::new("cmd-m", Minimize, None),
        ]);
    }
}

/// Open the page a Help action names. Bound on the app (the menu bar) and on
/// the main window's view (the palette, which on Linux is the only way to
/// them), through [`on_help`].
fn help(action: &dyn Action, cx: &mut App) {
    let guide = format!("{REPO}/blob/main/docs/guide.md");
    let a = action.as_any();
    let url = if a.is::<OpenGuide>() {
        guide
    } else if a.is::<OpenShortcuts>() {
        format!("{guide}#keyboard-and-mouse")
    } else if a.is::<OpenRepo>() {
        REPO.into()
    } else if a.is::<OpenIssues>() {
        format!("{REPO}/issues")
    } else if a.is::<OpenReleases>() {
        format!("{REPO}/releases")
    } else {
        return;
    };
    cx.open_url(&url);
}

/// The Help actions on a view, so the palette lists them.
pub fn on_help(d: Stateful<Div>) -> Stateful<Div> {
    d.on_action(|a: &OpenGuide, _, cx| help(a, cx))
        .on_action(|a: &OpenShortcuts, _, cx| help(a, cx))
        .on_action(|a: &OpenRepo, _, cx| help(a, cx))
        .on_action(|a: &OpenIssues, _, cx| help(a, cx))
        .on_action(|a: &OpenReleases, _, cx| help(a, cx))
        .on_action(|_: &crate::About, _, cx| crate::about::show(cx))
}

/// Build the menu bar. macOS shows each item's shortcut from the keymap as
/// it is now, so this runs again after the shortcuts change.
pub fn set_menus(cx: &mut App) {
    let app = channel::app_name();
    // The Edit items send copy:/paste:/… down the responder chain, which is
    // the only way the browser's WKWebView gets ⌘C/⌘V/⌘A/⌘X. gpui views
    // handle those keys through their own bindings before the menu sees them.
    cx.set_menus(vec![
        Menu {
            name: app.into(),
            items: vec![
                MenuItem::action(format!("About {app}"), About),
                MenuItem::action("Check for Updates…", crate::update::CheckForUpdates),
                MenuItem::separator(),
                MenuItem::action("Settings…", OpenSettings),
                MenuItem::action("Open Configuration File", OpenConfig),
                MenuItem::action("Open init.lua", OpenInitLua),
                MenuItem::action("Reload Configuration", ReloadConfig),
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
                MenuItem::action("New Claude Tab", NewClaudeChat),
                MenuItem::action(
                    "Switch Claude Code: Terminal ↔ Claude Tab",
                    ToggleClaudeView,
                ),
                MenuItem::separator(),
                MenuItem::action("Split Right", SplitRight),
                MenuItem::action("Split Down", SplitDown),
                MenuItem::separator(),
                MenuItem::action("Rename Tab…", RenameTab),
                MenuItem::action("Rename Workspace…", RenameWorkspace),
                MenuItem::action("Edit Workspace Description…", EditDescription),
                MenuItem::action("Restart Shell", terminal::RestartShell),
                MenuItem::separator(),
                MenuItem::action("Close Tab", CloseTab),
                MenuItem::action("Close Workspace", CloseWorkspace),
                MenuItem::separator(),
                MenuItem::action("Command Palette…", CommandPalette),
                MenuItem::action("Go to Workspace…", WorkspaceSwitcher),
                MenuItem::action("Go to Merge Request…", crate::GoToMergeRequest),
                MenuItem::action("Review Changes", crate::ReviewChanges),
                MenuItem::action("Review Changes in Hunk", crate::ReviewInHunk),
                MenuItem::action("Open Workspace in Neovim", crate::OpenInNeovim),
                MenuItem::action("Open Workspace in Editor", crate::OpenInEditor),
                MenuItem::separator(),
                MenuItem::action("Reopen Previous Session", ReopenSession),
                MenuItem::action("Import cmux Session", ImportCmuxSession),
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
                MenuItem::action("Zoom In", browser::ZoomIn),
                MenuItem::action("Zoom Out", browser::ZoomOut),
                MenuItem::action("Actual Size", browser::ActualSize),
                MenuItem::action("Focus Address Bar", FocusAddressBar),
                MenuItem::separator(),
                MenuItem::action("Toggle Developer Tools", browser::ToggleDevTools),
                MenuItem::action("Show JavaScript Console", browser::ShowConsole),
                MenuItem::separator(),
                MenuItem::action("Show Downloads", browser::ToggleDownloads),
            ],
        },
        Menu {
            name: "Notifications".into(),
            items: vec![
                MenuItem::action("Show Notifications", ToggleNotifications),
                MenuItem::action("Jump to Latest Unread", JumpToUnread),
                MenuItem::action("Toggle Unread", ToggleUnread),
                MenuItem::action("Toggle Workspace Unread", ToggleWorkspaceUnread),
                MenuItem::action(
                    "Mark as Oldest Unread and Jump to Next",
                    MarkOldestUnreadAndJumpNext,
                ),
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
                MenuItem::action("Move Workspace Up", MoveWorkspaceUp),
                MenuItem::action("Move Workspace Down", MoveWorkspaceDown),
                MenuItem::submenu(Menu {
                    name: "Workspace Groups".into(),
                    items: vec![
                        MenuItem::action("New Group from Workspace", NewGroup),
                        MenuItem::action("Group Workspaces by Folder", GroupByFolder),
                        MenuItem::action("Collapse or Expand Group", ToggleGroup),
                        MenuItem::action("Ungroup Workspaces", Ungroup),
                    ],
                }),
                MenuItem::action("Next Tab", NextTab),
                MenuItem::action("Previous Tab", PrevTab),
                MenuItem::separator(),
                MenuItem::action("Focus Pane Left", FocusLeft),
                MenuItem::action("Focus Pane Right", FocusRight),
                MenuItem::action("Focus Pane Up", FocusUp),
                MenuItem::action("Focus Pane Down", FocusDown),
                MenuItem::action("Focus Last Pane", LastPane),
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
