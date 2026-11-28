//! Actions more than one kind of pane handles, so they belong to neither
//! pane's module: ⌘F and its steps (terminals and browser pages, each with
//! its `crate::find_bar`), and Reload Page (browser panes, and
//! terminal-browser tabs, which are terminals).

use gpui::actions;

actions!(
    pane,
    [Find, FindNext, FindPrevious, UseSelectionForFind, Reload]
);
