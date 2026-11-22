//! The control socket describes itself: every method, what it does, its
//! parameters as JSON Schema and the CLI command that calls it, for
//! `abromux rpc list` and `abromux rpc describe <method>` (as Rex's
//! `rex api`). The running app answers (`system.methods`), so it always
//! matches the app the CLI talks to.
//!
//! A test checks this table against the methods `server.rs` handles, so a
//! new method can't go undescribed.
//!
//! Parameters are written compactly: `name[?]:type what it is`, separated
//! by `;`. `?` makes it optional. Types are JSON Schema's (`string`,
//! `integer`, `number`, `boolean`, `array`, `object`), plus `ref` (a ref
//! like `surface:3`, a 0-based index, or a name). `@surface`, `@pane`,
//! `@workspace` and `@browser` stand for the usual target parameters.

use serde_json::{Map, Value, json};

pub struct Method {
    pub name: &'static str,
    /// The `abromux` command that calls it, if any.
    pub cli: &'static str,
    pub doc: &'static str,
    pub params: &'static str,
}

const fn m(
    name: &'static str,
    cli: &'static str,
    doc: &'static str,
    params: &'static str,
) -> Method {
    Method {
        name,
        cli,
        doc,
        params,
    }
}

/// The shared target parameters.
fn expand(alias: &str) -> Option<&'static str> {
    Some(match alias {
        "@workspace" => {
            "workspace?:ref The workspace (default: the caller's, else the selected one)"
        }
        "@surface" => {
            "surface?:ref The tab (default: the caller, else the focused tab of --workspace or the selected workspace);\
             workspace?:ref Look in this workspace (for an index or the default)"
        }
        "@pane" => {
            "pane?:ref The pane (default: the one holding --surface or the caller);\
             surface?:ref A tab in the pane;\
             workspace?:ref Look in this workspace (for an index or the default)"
        }
        "@browser" => {
            "surface?:ref The browser tab (default: the workspace's focused browser, else its first);\
             workspace?:ref The workspace to look in"
        }
        _ => return None,
    })
}

const NEW_TAB: &str = "type?:string terminal (default), browser, markdown, claude, or a plugin's tab type such as gitlab;\
    url?:string For a browser;\
    path?:string For a markdown tab: the absolute path of the file;\
    command?:string For a terminal: typed into its shell, then Return;\
    cwd?:string For a terminal or Claude tab: where it runs;\
    session?:string For a Claude tab: the conversation to resume;\
    continue?:boolean For a Claude tab: resume the folder's latest conversation;\
    model?:string For a Claude tab: opus, sonnet, haiku, fable, default, or a full model name;\
    effort?:string For a Claude tab: low, medium, high, xhigh or max;\
    mode?:string For a Claude tab: ask, accept-edits, plan, auto, dont-ask or bypass;\
    prompt?:string For a Claude tab: sent as the first message;\
    label?:string Name the tab, to target it by that name later;\
    focus?:boolean Focus it (default true)";

pub const METHODS: &[Method] = &[
    m(
        "system.ping",
        "ping",
        "Check that the app answers: \"pong\".",
        "",
    ),
    m(
        "system.identify",
        "identify",
        "The socket, the caller's workspace and surface, and what's focused.",
        "",
    ),
    m(
        "system.methods",
        "rpc list",
        "Every method of the control socket, with its parameters (this list).",
        "method?:string Just this one",
    ),
    m(
        "workspace.list",
        "list-workspaces",
        "Every workspace: ref, name, cwd, git branch, status pills, progress, group.",
        "",
    ),
    m(
        "workspace.current",
        "current-workspace",
        "The caller's workspace, else the selected one.",
        "",
    ),
    m(
        "workspace.create",
        "new-workspace",
        "Open a workspace, with one terminal or a cmux-style layout.",
        "name?:string Its name;description?:string A description under the name;cwd?:string Where its terminals start;\
         command?:string Typed into its first terminal;focus?:boolean Select it (default true);group?:ref A sidebar group to put it in;\
         layout?:string A layout as JSON: splits, panes and tabs;\
         folder?:string Its own folder (absolute): $ABROMUX_WORKSPACE_DIR in its terminals, and kept with it (default cwd);\
         env?:object Environment variables for its terminals, kept with it;\
         provisional?:boolean The name is a stand-in that agents' auto-naming may replace",
    ),
    m(
        "workspace.select",
        "select-workspace",
        "Select a workspace.",
        "workspace:ref The workspace",
    ),
    m(
        "workspace.close",
        "close-workspace",
        "Close a workspace and everything in it.",
        "workspace:ref The workspace",
    ),
    m(
        "workspace.rename",
        "rename-workspace",
        "Rename a workspace.",
        "@workspace;title:string The new name",
    ),
    m(
        "workspace.action",
        "workspace-action",
        "One of the sidebar's workspace actions.",
        "@workspace;action:string rename, clear-name, set-description, clear-description, set-color, clear-color, move-up, move-down, move-top, move-bottom, move-to-cleanup (into the Cleanup group, last in the sidebar), close-others, close-above, close-below, mark-read, mark-unread;\
         title?:string For rename;description?:string For set-description;color?:string For set-color: a palette name or #rrggbb",
    ),
    m(
        "workspace.reorder",
        "reorder-workspace",
        "Move a workspace in the sidebar.",
        "@workspace;index?:integer To this position;before?:ref Before this workspace;after?:ref After this workspace;dry_run?:boolean Only say where it would go",
    ),
    m(
        "workspace.reorder_all",
        "reorder-workspaces",
        "Put workspaces in this order; unnamed ones follow in theirs.",
        "order:string Workspace refs, comma-separated;dry_run?:boolean Only say what would change",
    ),
    m(
        "workspace.tree",
        "tree",
        "Workspaces with their panes and tabs.",
        "@workspace;all?:boolean Every workspace",
    ),
    m(
        "group.list",
        "group list",
        "The sidebar's groups and their workspaces.",
        "",
    ),
    m(
        "group.create",
        "group create",
        "A new sidebar group holding the workspace.",
        "@workspace;name?:string Its name",
    ),
    m(
        "group.add",
        "group add",
        "Put a workspace in a group.",
        "group:ref The group;@workspace",
    ),
    m(
        "group.remove",
        "group remove",
        "Take a workspace out of its group.",
        "@workspace",
    ),
    m(
        "group.by_folder",
        "group by-folder",
        "Group the ungrouped workspaces that share a repository or folder.",
        "",
    ),
    m(
        "group.rename",
        "group rename",
        "Rename a group.",
        "group?:ref The group (default: the workspace's);@workspace;title:string The new name",
    ),
    m(
        "group.color",
        "group color",
        "Color a group.",
        "group?:ref The group (default: the workspace's);@workspace;color:string A palette name or #rrggbb",
    ),
    m(
        "group.collapse",
        "group collapse",
        "Fold a group.",
        "group?:ref The group (default: the workspace's);@workspace",
    ),
    m(
        "group.expand",
        "group expand",
        "Unfold a group.",
        "group?:ref The group (default: the workspace's);@workspace",
    ),
    m(
        "group.ungroup",
        "group ungroup",
        "Dissolve a group, keeping its workspaces.",
        "group?:ref The group (default: the workspace's);@workspace",
    ),
    m(
        "session.restore",
        "restore-session",
        "Reopen the workspaces of a saved session that aren't open.",
        "from?:string stable, nightly, nightly-<name>, cmux, or a session file (default: the one this launch restored)",
    ),
    m(
        "session.export",
        "restore-session --export",
        "Write the current session to a file.",
        "path:string The file;force?:boolean Overwrite it",
    ),
    m(
        "pane.list",
        "list-panes",
        "A workspace's panes.",
        "@workspace",
    ),
    m("pane.focus", "focus-pane", "Focus a pane.", "@pane"),
    m(
        "pane.last",
        "last-pane",
        "Focus the pane focused before this one.",
        "@workspace",
    ),
    m(
        "pane.swap",
        "swap-pane",
        "Swap a pane's tabs with its neighbour's.",
        "@pane;direction?:string left, right (default), up or down",
    ),
    m(
        "pane.resize",
        "resize-pane",
        "Move a pane's divider.",
        "@pane;direction?:string left, right (default), up or down;amount?:number Pixels (default panes.resizeStep)",
    ),
    m(
        "pane.create",
        "new-pane",
        "A new pane beside the workspace's focused one, with one tab.",
        "@workspace;direction?:string left, right (default), up or down",
    ),
    m(
        "pane.join",
        "join-pane",
        "Move a tab (or a pane's active tab) to the end of another pane, in any workspace.",
        "target_pane:ref Where it goes;surface?:ref The tab;pane?:ref Or this pane's active tab;focus?:boolean Focus it",
    ),
    m(
        "pane.zoom",
        "zoom-pane",
        "Make a pane fill its workspace, or put it back.",
        "@pane;state?:string on, off or toggle (default)",
    ),
    m(
        "surface.list",
        "list-pane-surfaces",
        "A pane's tabs, with each terminal's cwd, label, command state and program status.",
        "@pane",
    ),
    m(
        "surface.split",
        "new-split",
        "A new tab in a new pane beside a tab. The result has the new surface's ref.",
        "@surface;direction:string left, right, up or down",
    ),
    m(
        "surface.create",
        "new-surface",
        "A new tab in a pane.",
        "@pane",
    ),
    m(
        "surface.close",
        "close-surface",
        "Close a tab (ending its program). A pinned one is hidden instead, still running.",
        "@surface;unpin?:boolean Unpin and close a pinned one for good",
    ),
    m(
        "tab.pin",
        "tab pin",
        "Pin a tab to its workspace: the sidebar lists it, and closing it hides it.",
        "@surface",
    ),
    m(
        "tab.unpin",
        "tab unpin",
        "Unpin a tab; a hidden one closes.",
        "@surface",
    ),
    m(
        "tab.list",
        "tab list",
        "A workspace's pinned tabs, hidden ones too.",
        "@workspace",
    ),
    m(
        "surface.focus",
        "focus-surface",
        "Focus a tab and select its workspace.",
        "@surface",
    ),
    m(
        "surface.send_text",
        "send",
        "Type text into a terminal. \\n presses Return; also \\t, \\e and \\\\.",
        "@surface;text:string What to type",
    ),
    m(
        "surface.send_key",
        "send-key",
        "Press a key in a terminal.",
        "@surface;key:string enter, tab, escape, up, ctrl-c, alt-b, ...",
    ),
    m(
        "surface.rename",
        "rename-tab",
        "Name a tab; it can be targeted by that name. No title: back to the program's own.",
        "@surface;title?:string The name",
    ),
    m(
        "surface.respawn",
        "respawn-pane",
        "Kill a terminal's shell and start a fresh one in its directory.",
        "@surface;command?:string Run this instead",
    ),
    m(
        "surface.move",
        "move-surface",
        "Move a tab to another pane or workspace.",
        "@surface;pane?:ref To this pane;index?:integer At this position;before?:ref Before this tab;after?:ref After this tab;focus?:boolean Focus it",
    ),
    m(
        "surface.reorder",
        "reorder-surface",
        "Move a tab within its pane.",
        "@surface;index?:integer To this position;before?:ref Before this tab;after?:ref After this tab",
    ),
    m(
        "surface.split_off",
        "split-off",
        "Move a tab into a new pane beside its own.",
        "@surface;direction:string left, right, up or down;focus?:boolean Focus it",
    ),
    m(
        "surface.move_to_new_workspace",
        "move-tab-to-new-workspace",
        "Give a tab (or a pane's active tab) a workspace of its own.",
        "@surface;pane?:ref This pane's active tab;title?:string The new workspace's name;focus?:boolean Select it",
    ),
    m(
        "surface.flash",
        "trigger-flash",
        "Flash the pane holding a tab.",
        "@surface",
    ),
    m(
        "surface.clear_history",
        "clear-history",
        "Clear a terminal's screen and scrollback.",
        "@surface",
    ),
    m(
        "surface.read_text",
        "read-screen",
        "A terminal's screen as text.",
        "@surface;lines?:integer Only the last this many;scrollback?:boolean The scrollback too",
    ),
    m(
        "surface.read_selection",
        "read-selection",
        "The selected text, in a terminal or a page.",
        "@surface",
    ),
    m(
        "surface.wait",
        "wait",
        "Wait until the command running in a terminal finishes (from the shell integration's marks; without them, until the terminal closes), or with close until the tab closes. Returns exit_code; the CLI exits with it.",
        "@surface;close?:boolean Wait for the tab to close instead;after?:integer Wait for a command after this many had finished (from surface.list's command.finished), not one already done;\
         timeout?:number Give up after this many seconds",
    ),
    m(
        "surface.action",
        "tab-action",
        "One of the tab bar's actions on a tab.",
        "@surface;action:string rename, clear-name, close-left, close-right, close-others, new-terminal-right, new-browser-right, duplicate, move-to-new-workspace, reload, mark-read, mark-unread;\
         title?:string For rename;url?:string For new-browser-right;focus?:boolean Focus what it makes",
    ),
    m(
        "surface.plugin",
        "",
        "A plugin tab's own command (the GitLab tab's).",
        "@surface;command:string The command;args?:object Its arguments",
    ),
    m(
        "ui.prompt",
        "",
        "Ask in the command palette and wait for the answer: a line of text (kind input) or a choice from a list (kind pick). Lua scripts call it as abromux.ui.input / abromux.ui.pick. Returns answer (the text, the chosen item's id, or {text} for typed text with allow_other), or reason when there's none: cancelled, timeout or busy (another question is showing).",
        "kind:string input or pick;title:string The question;placeholder?:string Hint while the field is empty;\
         value?:string Text the field starts with;confirm?:string What Enter does, shown under the field;\
         items?:array For pick: ids as strings, or objects with id, title?, subtitle?, detail?;\
         allow_other?:boolean For pick: typed text that matches no item is an answer too;\
         timeout?:number Seconds to wait (default 300, at most 3600)",
    ),
    m(
        "events.subscribe",
        "events",
        "Turn the connection into a stream of events, one JSON object per line after the reply, until it closes. See `abromux events --help` for the events.",
        "events?:array Event names, or prefixes ending in . or * (default: all);workspace?:ref Only this workspace's;surface?:ref Only this tab's",
    ),
    m(
        "agent.report",
        "",
        "A coding agent's hook reporting (abromux hooks install sets these up).",
        "@surface;kind:string The agent;permission_mode?:string Its permission mode",
    ),
    m(
        "file.edit",
        "edit",
        "Open a file in editor.program (nvim or tode) in a pane beside a terminal.",
        "path:string The file;@surface;focus?:boolean Focus it",
    ),
    m(
        "browser.open",
        "browser open",
        "A browser split beside the caller.",
        "@surface;url?:string The page;direction?:string Where the split goes;focus?:boolean Focus it",
    ),
    m(
        "browser.navigate",
        "browser goto",
        "Go to a page.",
        "@browser;url:string The page",
    ),
    m("browser.back", "browser back", "Go back.", "@browser"),
    m(
        "browser.forward",
        "browser forward",
        "Go forward.",
        "@browser",
    ),
    m(
        "browser.reload",
        "browser reload",
        "Reload the page.",
        "@browser",
    ),
    m(
        "browser.zoom",
        "browser zoom",
        "Zoom the page, or say how far it's zoomed.",
        "@browser;zoom?:string in, out, reset, a factor like 1.25, or 125%",
    ),
    m(
        "browser.url",
        "browser url",
        "The page's address.",
        "@browser",
    ),
    m(
        "browser.eval",
        "browser eval",
        "Run JavaScript in the page (or the frame browser.frame picked); its value as JSON.",
        "@browser;script:string The script",
    ),
    m(
        "browser.snapshot",
        "browser snapshot",
        "The page as an accessibility tree, with a ref (e3) per element for the other commands.",
        "@browser;interactive?:boolean Only what can be interacted with;compact?:boolean Fewer details;selector?:string Only under this element",
    ),
    m(
        "browser.click",
        "browser click",
        "Click an element.",
        "@browser;selector:string A CSS selector or a ref from the last snapshot",
    ),
    m(
        "browser.dblclick",
        "browser dblclick",
        "Double-click an element.",
        "@browser;selector:string A CSS selector or a ref",
    ),
    m(
        "browser.hover",
        "browser hover",
        "Hover over an element.",
        "@browser;selector:string A CSS selector or a ref",
    ),
    m(
        "browser.focus",
        "browser focus",
        "Focus an element.",
        "@browser;selector:string A CSS selector or a ref",
    ),
    m(
        "browser.check",
        "browser check",
        "Check (or uncheck) a checkbox.",
        "@browser;selector:string A CSS selector or a ref;checked?:boolean false unchecks",
    ),
    m(
        "browser.scroll_into_view",
        "browser scroll-into-view",
        "Scroll an element into view.",
        "@browser;selector:string A CSS selector or a ref",
    ),
    m(
        "browser.fill",
        "browser fill",
        "Replace a field's value.",
        "@browser;selector:string A CSS selector or a ref;text:string The value",
    ),
    m(
        "browser.type",
        "browser type",
        "Type into a field key by key, after what's there.",
        "@browser;selector:string A CSS selector or a ref;text:string What to type",
    ),
    m(
        "browser.press",
        "browser press",
        "Press a key.",
        "@browser;key:string Enter, Tab, Escape, ArrowDown, ctrl+a, ...;selector?:string In this element",
    ),
    m(
        "browser.keydown",
        "browser keydown",
        "Just the key-down event.",
        "@browser;key:string The key;selector?:string In this element",
    ),
    m(
        "browser.keyup",
        "browser keyup",
        "Just the key-up event.",
        "@browser;key:string The key;selector?:string In this element",
    ),
    m(
        "browser.select",
        "browser select",
        "Choose options in a select.",
        "@browser;selector:string A CSS selector or a ref;values:array The options' values",
    ),
    m(
        "browser.scroll",
        "browser scroll",
        "Scroll the page or an element.",
        "@browser;dx?:number Pixels right;dy?:number Pixels down;selector?:string This element",
    ),
    m(
        "browser.get",
        "browser get",
        "Read something from the page.",
        "@browser;what:string url, title, text, html, value, attr, count, box or styles;selector?:string The element;name?:string For attr: the attribute;names?:array For styles: the properties",
    ),
    m(
        "browser.is",
        "browser is",
        "Whether an element is in a state.",
        "@browser;state:string visible, hidden, enabled, disabled, checked, editable or focused;selector:string The element",
    ),
    m(
        "browser.find",
        "browser find",
        "Find elements the way Playwright's locators do.",
        "@browser;by:string role, text, label, placeholder, alt, title or testid;value:string What to look for;name?:string For role: the accessible name;exact?:boolean Match exactly",
    ),
    m(
        "browser.storage",
        "browser storage",
        "The page's localStorage or sessionStorage.",
        "@browser;area:string local or session;op?:string get (default), set, remove or clear;key?:string The key;value?:string For set",
    ),
    m(
        "browser.console",
        "browser console",
        "What the page logged.",
        "@browser;level?:string Only this level;filter?:string Only entries with this text;limit?:integer The newest this many;clear?:boolean Then forget them",
    ),
    m(
        "browser.errors",
        "browser errors",
        "Uncaught exceptions and unhandled rejections.",
        "@browser;filter?:string Only entries with this text;limit?:integer The newest this many;clear?:boolean Then forget them",
    ),
    m(
        "browser.network",
        "browser network",
        "Requests the page made.",
        "@browser;filter?:string Only these;limit?:integer The newest this many;clear?:boolean Then forget them",
    ),
    m(
        "browser.frame",
        "browser frame",
        "Run later commands in an iframe (cross-origin too), or in the page again.",
        "@browser;selector?:string The iframe (or main for the page);name?:string Or by name;url?:string Or by address",
    ),
    m(
        "browser.cookies",
        "browser cookies",
        "The page's cookies, HttpOnly too: get, set or clear.",
        "@browser;op?:string get (default), set or clear;name?:string The cookie;value?:string For set;domain?:string;path?:string;expires?:integer Unix seconds;\
         secure?:boolean;http_only?:boolean;same_site?:string Strict, Lax or None;url?:string Another site's;all?:boolean Every site's",
    ),
    m(
        "browser.state",
        "browser state",
        "Save or load cookies and the page's localStorage (Playwright's storageState format).",
        "@browser;op:string save or load;path:string The file",
    ),
    m(
        "browser.tab",
        "browser tab",
        "The browser tabs in a browser's pane.",
        "@browser;op?:string list (default), new, switch or close;target?:string For switch and close: an index or surface ref;url?:string For new",
    ),
    m(
        "browser.downloads",
        "browser downloads",
        "This session's downloads.",
        "op?:string list (default), clear or cancel;id?:integer For cancel",
    ),
    m(
        "browser.wait_download",
        "browser wait-download",
        "Wait for the running download, else the next, to finish; its path.",
        "id?:integer This one;path?:string Move it here (a file or folder);timeout?:integer Milliseconds (default 30000)",
    ),
    m(
        "browser.wait",
        "browser wait",
        "Wait for an element, text, address, load state or JavaScript condition.",
        "@browser;selector?:string An element;state?:string visible (default), hidden, attached or detached;text?:string Text on the page;url?:string Part of the address;\
         load?:string interactive or complete;js?:string A condition;timeout?:integer Milliseconds (default 10000)",
    ),
    m(
        "browser.screenshot",
        "browser screenshot",
        "A PNG of the page (it must be on screen).",
        "@browser;path?:string The file (default: one in $TMPDIR)",
    ),
    m(
        "notification.create",
        "notify",
        "A notification from a terminal: rings its pane, and a banner while the app is in the background.",
        "@surface;title?:string;subtitle?:string;body?:string",
    ),
    m(
        "notification.list",
        "list-notifications",
        "The notifications, newest first.",
        "",
    ),
    m(
        "notification.open",
        "open-notification",
        "Show a notification's terminal and mark it read.",
        "id:ref notification:N or N",
    ),
    m(
        "notification.dismiss",
        "dismiss-notification",
        "Remove a notification, or every read one.",
        "id?:ref notification:N or N;all_read?:boolean Every read one",
    ),
    m(
        "notification.mark_read",
        "mark-notification-read",
        "Mark notifications read.",
        "id?:ref This one;workspace?:ref This workspace's;surface?:ref This tab's;all?:boolean All of them",
    ),
    m(
        "notification.jump_unread",
        "jump-to-unread",
        "Show the newest unread notification's terminal.",
        "",
    ),
    m(
        "notification.clear",
        "clear-notifications",
        "Remove every notification.",
        "",
    ),
    m(
        "status.set",
        "set-status",
        "A pill under the workspace in the sidebar.",
        "@workspace;key:string Its name (setting it again replaces it);value:string Its text;icon?:string One of abromux's icons;color?:string #rrggbb",
    ),
    m(
        "status.clear",
        "clear-status",
        "Remove a pill.",
        "@workspace;key:string Its name",
    ),
    m(
        "status.list",
        "list-status",
        "A workspace's pills: scripts', agents' and programs' (OSC 7501).",
        "@workspace",
    ),
    m(
        "progress.set",
        "set-progress",
        "A progress bar under the workspace.",
        "@workspace;value:number 0.0 to 1.0;label?:string Text above it",
    ),
    m(
        "progress.clear",
        "clear-progress",
        "Remove the progress bar.",
        "@workspace",
    ),
    m(
        "log.add",
        "log",
        "A line in the workspace's log; the sidebar shows the newest.",
        "@workspace;message:string The text;level?:string info (default), progress, success, warning or error;source?:string Who says it",
    ),
    m(
        "log.list",
        "list-log",
        "The workspace's log.",
        "@workspace;limit?:integer The newest this many",
    ),
    m(
        "log.clear",
        "clear-log",
        "Empty the workspace's log.",
        "@workspace",
    ),
    m(
        "app.reload_config",
        "reload-config",
        "Read the config file again.",
        "",
    ),
    m(
        "app.sidebar",
        "sidebar",
        "Show or hide the sidebar.",
        "state?:string toggle (default), show or hide",
    ),
    m(
        "app.quit",
        "quit",
        "Quit the app (held shells keep running).",
        "",
    ),
    m(
        "window.focus",
        "focus-window",
        "Bring the window to the front.",
        "",
    ),
    m(
        "mr.list",
        "mr list",
        "Cached GitLab merge requests.",
        "group?:string mine, recent or workspace (default: all)",
    ),
    m(
        "mr.open",
        "mr open",
        "A merge request in a tab of the caller's workspace.",
        "ref:string Its URL or group/project!iid",
    ),
    m(
        "mr.refresh",
        "mr refresh",
        "Check the merge requests now, or load one again.",
        "ref?:string This one",
    ),
    m(
        "mr.panel",
        "mr panel",
        "Show or hide the merge requests panel.",
        "state?:string toggle (default), show or hide",
    ),
    m(
        "link.list",
        "link list",
        "A workspace's sidebar list: its pinned tabs, then suggested links.",
        "@workspace",
    ),
    m(
        "link.add",
        "link add",
        "Open a link in a hidden pinned tab of a workspace, unless a tab shows it.",
        "@workspace;ref:string A URL, PAY-123 or group/project!iid",
    ),
    m(
        "link.remove",
        "link remove",
        "Unpin a listed tab, or stop suggesting a link.",
        "@workspace;ref:string Its number, key, URL or tab ref",
    ),
    m(
        "link.open",
        "link open",
        "Show a listed link: its tab, else a new pinned one.",
        "@workspace;ref?:string Its number, key or URL;all?:boolean Every one;focus?:boolean Focus it",
    ),
    m(
        "debug.frame",
        "",
        "Debugging: the last frame's timings.",
        "refresh?:boolean Draw one first",
    ),
    m(
        "debug.frames",
        "",
        "Debugging: record frame timings.",
        "record?:boolean Start (true) or stop and report (false)",
    ),
    m(
        "debug.input",
        "",
        "Debugging: replay input steps.",
        "steps:array The steps",
    ),
];

/// Methods the new-tab parameters apply to.
const NEW_TAB_METHODS: &[&str] = &["surface.split", "surface.create", "pane.create"];

/// Each method's namespace (`surface` of `surface.split`), once.
pub fn namespaces() -> Vec<&'static str> {
    let mut out: Vec<&str> = vec![];
    for m in METHODS {
        let ns = m.name.split('.').next().unwrap_or(m.name);
        if !out.contains(&ns) {
            out.push(ns);
        }
    }
    out
}

pub fn find(name: &str) -> Option<&'static Method> {
    METHODS.iter().find(|m| m.name == name)
}

/// `(name, type, required, doc)` for each of `m`'s parameters.
fn params(m: &Method) -> Vec<(String, String, bool, String)> {
    let mut list = m.params.to_string();
    if NEW_TAB_METHODS.contains(&m.name) {
        list = format!("{list};{NEW_TAB}");
    }
    let mut out: Vec<(String, String, bool, String)> = vec![];
    for part in list.split(';').map(str::trim).filter(|p| !p.is_empty()) {
        let parts: Vec<&str> = match expand(part) {
            Some(e) => e.split(';').map(str::trim).collect(),
            None => vec![part],
        };
        for p in parts {
            let (head, doc) = p.split_once(' ').unwrap_or((p, ""));
            let (name, ty) = head.split_once(':').unwrap_or((head, "string"));
            let (name, required) = match name.strip_suffix('?') {
                Some(n) => (n, false),
                None => (name, true),
            };
            if out.iter().any(|(n, ..)| n == name) {
                continue;
            }
            out.push((name.into(), ty.into(), required, doc.trim().into()));
        }
    }
    out
}

/// One method as JSON: name, description, CLI command, and its parameters
/// as a JSON Schema.
pub fn describe(m: &Method) -> Value {
    let mut props = Map::new();
    let mut required = vec![];
    for (name, ty, req, doc) in params(m) {
        // A ref is a string: `kind:N`, a 0-based index, or a name.
        let mut schema = match ty.as_str() {
            "ref" => json!({ "type": "string" }),
            t => json!({ "type": t }),
        };
        if !doc.is_empty() {
            schema["description"] = json!(doc);
        }
        if ty == "ref" {
            let kind = match name.as_str() {
                "target_pane" => "pane",
                "id" => "notification",
                n => n,
            };
            let named = if matches!(kind, "workspace" | "surface" | "group") {
                ", or a name"
            } else {
                ""
            };
            let d = schema["description"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            schema["description"] = json!(
                format!("{d} ({kind}:N, a 0-based index{named})")
                    .trim()
                    .to_string()
            );
        }
        props.insert(name.clone(), schema);
        if req {
            required.push(name);
        }
    }
    json!({
        "name": m.name,
        "description": m.doc,
        "cli": (!m.cli.is_empty()).then(|| format!("abromux {}", m.cli)),
        "input_schema": {
            "type": "object",
            "properties": props,
            "required": required,
        },
    })
}

/// `system.methods`: every method, or just `method`.
pub fn methods(p: &Value) -> anyhow::Result<Value> {
    match p.get("method").and_then(Value::as_str) {
        Some(name) => find(name)
            .map(describe)
            .ok_or_else(|| anyhow::anyhow!("no method {name:?} (see `abromux rpc list`)")),
        None => Ok(json!(METHODS.iter().map(describe).collect::<Vec<_>>())),
    }
}

/// `rpc describe`, for reading.
pub fn describe_text(v: &Value) -> String {
    let mut out = format!(
        "{}\n\n{}\n",
        v["name"].as_str().unwrap_or_default(),
        v["description"].as_str().unwrap_or_default()
    );
    if let Some(cli) = v["cli"].as_str() {
        out.push_str(&format!("\nCLI: {cli}\n"));
    }
    let schema = &v["input_schema"];
    let required: Vec<&str> = schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let props = schema["properties"].as_object();
    if props.is_none_or(|p| p.is_empty()) {
        out.push_str("\nNo parameters.\n");
    } else {
        out.push_str("\nParameters:\n");
        for (name, s) in props.into_iter().flatten() {
            let ty = s["type"].as_str().unwrap_or_default();
            let req = if required.contains(&name.as_str()) {
                ""
            } else {
                "?"
            };
            out.push_str(&format!(
                "  {:<22} {}\n",
                format!("{name}{req}: {ty}"),
                s["description"].as_str().unwrap_or_default()
            ));
        }
    }
    out.push_str(&format!(
        "\nInput schema:\n{}\n",
        serde_json::to_string_pretty(schema).unwrap_or_default()
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The methods `server.rs` handles, from its `match method` arms.
    fn handled() -> Vec<String> {
        let src = include_str!("server.rs");
        let start = src.find("let v = match method {").expect("the match");
        let body = &src[start..];
        let end = body
            .find("_ => return Err(MethodNotFound")
            .expect("its end");
        let mut out = vec![];
        for line in body[..end].lines() {
            let t = line.trim_start();
            // An arm's pattern: `"a.b" =>`, `"a.b" | "c.d" =>`, or one per line.
            if line.len() - t.len() != 12 || !t.starts_with('"') && !t.starts_with("| \"") {
                continue;
            }
            for name in t.split('"').skip(1).step_by(2) {
                if name.contains('.')
                    && name
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c == '.' || c == '_')
                {
                    out.push(name.to_string());
                }
            }
        }
        out
    }

    #[test]
    fn every_method_is_described_and_every_description_is_handled() {
        let handled = handled();
        assert!(handled.len() > 80, "found {} methods", handled.len());
        for h in &handled {
            assert!(find(h).is_some(), "{h} is handled but not in api.rs");
        }
        for m in METHODS {
            assert!(
                handled.iter().any(|h| h == m.name),
                "{} is described in api.rs but not handled",
                m.name
            );
        }
    }

    #[test]
    fn describes_parameters_as_json_schema() {
        let v = describe(find("surface.split").unwrap());
        assert_eq!(v["cli"], "abromux new-split");
        let s = &v["input_schema"];
        assert_eq!(s["required"], json!(["direction"]));
        assert_eq!(s["properties"]["surface"]["type"], "string");
        assert!(
            s["properties"]["workspace"]["description"]
                .as_str()
                .unwrap()
                .contains("workspace:N")
        );
        assert_eq!(s["properties"]["label"]["type"], "string");
        assert_eq!(s["properties"]["focus"]["type"], "boolean");
        let text = describe_text(&v);
        assert!(text.contains("direction: string"), "{text}");
        assert!(methods(&json!({ "method": "nope" })).is_err());
    }
}
