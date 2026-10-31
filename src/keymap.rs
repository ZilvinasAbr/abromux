//! The app's keyboard shortcuts, and the ones the config file sets.
//!
//! `ACTIONS` names every action a shortcut can run, with its default keys
//! and the key context each binding lives in. The config file's
//! `"shortcuts"` map replaces an action's keys by name, and its
//! `"commands"` add palette commands, each with an optional shortcut of its
//! own (`settings.rs`). `apply` rebuilds the keymap from the two, on launch
//! and on every config reload.
//!
//! Keys are gpui keystrokes (`cmd-shift-p`), or macOS symbols (`⌘⇧P`).
//! Several keystrokes separated by spaces are a chord (`ctrl-a h`).

use crate::settings::Settings;
use gpui::{Action, App, Global, KeyBinding, KeyBindingContextPredicate, Keystroke};
use std::rc::Rc;

/// An action a shortcut can run.
pub struct Act {
    /// Its key in the config file's `"shortcuts"`.
    pub name: &'static str,
    /// Its title in the palette; the ones without one aren't listed there.
    pub title: Option<&'static str>,
    pub action: fn() -> Box<dyn Action>,
    /// Default keys per key context. A shortcut set in the config file
    /// replaces all of them, and is bound in each of these contexts.
    pub bindings: &'static [(Option<&'static str>, &'static [&'static str])],
}

macro_rules! acts {
    ($( $name:literal, $title:expr, $action:expr, [$( ($ctx:expr, [$($k:literal),*]) ),*] );* $(;)?) => {
        &[$(Act {
            name: $name,
            title: $title,
            action: || Box::new($action),
            bindings: &[$(($ctx, &[$($k),*])),*],
        }),*]
    };
}

const T: Option<&str> = Some("Terminal");
const B: Option<&str> = Some("Browser");
/// The address bar and the browser's find field.
const BI: Option<&str> = Some("Browser > Input");

/// Every rebindable action, in palette order.
#[rustfmt::skip]
pub const ACTIONS: &[Act] = {
    use crate::*;
    acts![
        "newWorkspace", Some("New Workspace"), NewWorkspace, [(None, ["cmd-n"])];
        "renameWorkspace", Some("Rename Workspace…"), RenameWorkspace, [(None, ["cmd-shift-r"])];
        "editWorkspaceDescription", Some("Edit Workspace Description…"), EditDescription,
            [(None, ["alt-cmd-e"])];
        "closeWorkspace", Some("Close Workspace"), CloseWorkspace, [(None, ["cmd-shift-w"])];
        "moveWorkspaceUp", Some("Move Workspace Up"), MoveWorkspaceUp, [(None, [])];
        "moveWorkspaceDown", Some("Move Workspace Down"), MoveWorkspaceDown, [(None, [])];
        "newWorkspaceGroup", Some("New Group from Workspace"), NewGroup, [(None, [])];
        "groupWorkspacesByFolder", Some("Group Workspaces by Folder"), GroupByFolder, [(None, [])];
        "ungroupWorkspaces", Some("Ungroup Workspaces"), Ungroup, [(None, [])];
        "toggleWorkspaceGroup", Some("Collapse or Expand Group"), ToggleGroup, [(None, [])];
        "reopenPreviousSession", Some("Reopen Previous Session"), ReopenSession,
            [(None, ["cmd-shift-o"])];
        "nextWorkspace", Some("Next Workspace"), NextWorkspace, [(None, ["ctrl-cmd-]"])];
        "previousWorkspace", Some("Previous Workspace"), PrevWorkspace, [(None, ["ctrl-cmd-["])];
        "newTerminal", Some("New Terminal Tab"), NewTerminal, [(None, ["cmd-t"])];
        "newBrowser", Some("New Browser Tab"), NewBrowser, [(None, ["cmd-shift-l"])];
        "splitRight", Some("Split Right"), SplitRight, [(None, ["cmd-d"])];
        "splitDown", Some("Split Down"), SplitDown, [(None, ["cmd-shift-d"])];
        "renameTab", Some("Rename Tab…"), RenameTab, [(None, ["cmd-r"])];
        "closeTab", Some("Close Tab"), CloseTab, [(None, ["cmd-w"])];
        // ⌘⇧] arrives from macOS as cmd-} (shift folded into the key), so
        // "cmd-shift-]" would never match. `normalize` folds it the same way.
        "nextTab", Some("Next Tab"), NextTab, [(None, ["cmd-}", "ctrl-tab"])];
        "previousTab", Some("Previous Tab"), PrevTab, [(None, ["cmd-{", "ctrl-shift-tab"])];
        "lastPane", Some("Focus Last Pane"), LastPane, [(None, [])];
        "focusLeft", Some("Focus Pane Left"), FocusLeft, [(None, ["alt-cmd-left"])];
        "focusRight", Some("Focus Pane Right"), FocusRight, [(None, ["alt-cmd-right"])];
        "focusUp", Some("Focus Pane Up"), FocusUp, [(None, ["alt-cmd-up"])];
        "focusDown", Some("Focus Pane Down"), FocusDown, [(None, ["alt-cmd-down"])];
        "swapLeft", Some("Swap Pane Left"), SwapLeft, [(None, [])];
        "swapRight", Some("Swap Pane Right"), SwapRight, [(None, [])];
        "swapUp", Some("Swap Pane Up"), SwapUp, [(None, [])];
        "swapDown", Some("Swap Pane Down"), SwapDown, [(None, [])];
        "resizeLeft", Some("Resize Pane Left"), ResizeLeft, [(None, ["ctrl-shift-h"])];
        "resizeRight", Some("Resize Pane Right"), ResizeRight, [(None, ["ctrl-shift-l"])];
        "resizeUp", Some("Resize Pane Up"), ResizeUp, [(None, ["ctrl-shift-k"])];
        "resizeDown", Some("Resize Pane Down"), ResizeDown, [(None, ["ctrl-shift-j"])];
        "flashPane", Some("Flash Focused Pane"), FlashPane, [(None, ["cmd-shift-h"])];
        "toggleZoom", Some("Toggle Pane Zoom"), ToggleZoom, [(None, ["cmd-shift-enter"])];
        "toggleSidebar", Some("Toggle Sidebar"), ToggleSidebar, [(None, ["cmd-b"])];
        "showNotifications", Some("Show Notifications"), ToggleNotifications, [(None, ["cmd-i"])];
        "jumpToUnread", Some("Jump to Latest Unread"), JumpToUnread, [(None, ["cmd-shift-u"])];
        "commandPalette", None, CommandPalette, [(None, ["cmd-shift-p"])];
        "workspaceSwitcher", None, WorkspaceSwitcher, [(None, ["cmd-p"])];
        // ⌘⇧, arrives as cmd-<.
        "reloadConfig", Some("Reload Config"), ReloadConfig, [(None, ["cmd-<"])];
        "persistenceAbromux", Some("Persistence: abromux (shells keep running, default)"),
            PersistenceAbromux, [(None, [])];
        "persistenceCmuxLike", Some("Persistence: cmux-like (shells end on quit)"),
            PersistenceCmuxLike, [(None, [])];
        "openSettings", Some("Settings…"), OpenSettings, [(None, ["cmd-,"])];
        "copy", Some("Copy"), terminal::Copy, [(T, ["cmd-c"])];
        "paste", Some("Paste"), terminal::Paste, [(T, ["cmd-v"])];
        "find", Some("Find…"), terminal::Find, [(T, ["cmd-f"]), (B, ["cmd-f"]), (BI, ["cmd-f"])];
        "findNext", Some("Find Next"), terminal::FindNext, [(T, ["cmd-g"]), (B, ["cmd-g"])];
        "findPrevious", Some("Find Previous"), terminal::FindPrevious,
            [(T, ["cmd-shift-g", "alt-cmd-g"]), (B, ["cmd-shift-g", "alt-cmd-g"])];
        "useSelectionForFind", Some("Use Selection for Find"), terminal::UseSelectionForFind,
            [(T, ["cmd-e"]), (B, ["cmd-e"])];
        "clearScrollback", Some("Clear Scrollback"), terminal::ClearScrollback, [(T, ["cmd-k"])];
        "restartShell", Some("Restart Shell"), terminal::RestartShell, [(T, [])];
        "increaseFontSize", Some("Increase Font Size"), terminal::IncreaseFontSize,
            [(T, ["cmd-=", "cmd-+"])];
        "decreaseFontSize", Some("Decrease Font Size"), terminal::DecreaseFontSize, [(T, ["cmd--"])];
        "resetFontSize", Some("Reset Font Size"), terminal::ResetFontSize, [(T, ["cmd-0"])];
        "focusAddressBar", Some("Focus Address Bar"), FocusAddressBar, [(None, ["cmd-l"])];
        "back", Some("Back"), browser::Back, [(B, ["cmd-["]), (BI, ["cmd-["])];
        "forward", Some("Forward"), browser::Forward, [(B, ["cmd-]"]), (BI, ["cmd-]"])];
        "reloadPage", Some("Reload Page"), browser::Reload, [(B, ["cmd-r"])];
        "toggleDevTools", Some("Toggle Developer Tools"), browser::ToggleDevTools, [(B, ["alt-cmd-i"])];
        "showConsole", Some("Show JavaScript Console"), browser::ShowConsole, [(B, ["alt-cmd-c"])];
        "quit", Some("Quit"), Quit, [(None, ["cmd-q"])];
        "workspace1", None, Ws1, [(None, ["cmd-1"])];
        "workspace2", None, Ws2, [(None, ["cmd-2"])];
        "workspace3", None, Ws3, [(None, ["cmd-3"])];
        "workspace4", None, Ws4, [(None, ["cmd-4"])];
        "workspace5", None, Ws5, [(None, ["cmd-5"])];
        "workspace6", None, Ws6, [(None, ["cmd-6"])];
        "workspace7", None, Ws7, [(None, ["cmd-7"])];
        "workspace8", None, Ws8, [(None, ["cmd-8"])];
        "workspace9", None, Ws9, [(None, ["cmd-9"])];
        "tab1", None, Tab1, [(None, ["ctrl-1"])];
        "tab2", None, Tab2, [(None, ["ctrl-2"])];
        "tab3", None, Tab3, [(None, ["ctrl-3"])];
        "tab4", None, Tab4, [(None, ["ctrl-4"])];
        "tab5", None, Tab5, [(None, ["ctrl-5"])];
        "tab6", None, Tab6, [(None, ["ctrl-6"])];
        "tab7", None, Tab7, [(None, ["ctrl-7"])];
        "tab8", None, Tab8, [(None, ["ctrl-8"])];
        "tab9", None, Tab9, [(None, ["ctrl-9"])];
    ]
};

pub fn find(name: &str) -> Option<&'static Act> {
    ACTIONS.iter().find(|a| a.name == name)
}

/// Runs `"commands"[index]` of the config file, or of the project file
/// `project` (`settings::find_project`).
#[derive(Clone, PartialEq, serde::Deserialize, gpui::Action)]
#[action(namespace = abromux, no_json)]
pub struct RunCommand {
    pub index: usize,
    #[serde(default)]
    pub project: Option<std::path::PathBuf>,
}

/// The keymap before `apply` added the shortcuts: gpui-component's, the
/// menu's and the fixed ones (palette navigation, keys kept for the PTY).
struct Base(Vec<KeyBinding>);

impl Global for Base {}

/// Bind every action's keys, with the config file's shortcuts and commands
/// in place of the defaults. The first call remembers the keymap as it was,
/// so later calls (on reload) start again from it.
pub fn apply(settings: &Settings, cx: &mut App) {
    if !cx.has_global::<Base>() {
        let keymap = cx.key_bindings();
        let base = keymap.borrow().bindings().cloned().collect();
        cx.set_global(Base(base));
    }
    let mut bindings = cx.global::<Base>().0.clone();
    bindings.extend(bindings_for(settings));
    // Keys a terminal-browser tab keeps: last, so they win over the
    // Terminal bindings in the same view, and bound to nothing, so they fall
    // through to the terminal, which sends them on.
    bindings.extend(crate::terminal_browser::KEYS.iter().map(|k| {
        KeyBinding::new(k, gpui::NoAction, Some(crate::terminal_browser::KEY_CONTEXT))
    }));
    cx.clear_key_bindings();
    cx.bind_keys(bindings);
}

/// One keystroke sequence bound to an action in a context.
struct Bind {
    keys: Vec<Keystroke>,
    action: Box<dyn Action>,
    context: Option<String>,
}

/// The shortcuts' bindings, user ones last so they win over any default
/// that's left. A key the config file gives an action is taken away from
/// whichever default had it: in the same context, or in every context when
/// it's set app-wide.
fn bindings_for(settings: &Settings) -> Vec<KeyBinding> {
    let mut defaults = vec![];
    let mut user = vec![];
    for act in ACTIONS {
        match settings.shortcuts.get(act.name) {
            None => {
                for (ctx, keys) in act.bindings {
                    for k in *keys {
                        defaults.push(Bind {
                            keys: parse_seq(k).expect("default shortcut parses"),
                            action: (act.action)(),
                            context: ctx.map(Into::into),
                        });
                    }
                }
            }
            Some(s) => {
                let contexts: Vec<Option<String>> = match &s.when {
                    Some(w) => vec![Some(w.clone())],
                    None => act
                        .bindings
                        .iter()
                        .map(|(c, _)| c.map(Into::into))
                        .collect(),
                };
                for keys in &s.keys {
                    for ctx in &contexts {
                        user.push(Bind {
                            keys: keys.clone(),
                            action: (act.action)(),
                            context: ctx.clone(),
                        });
                    }
                }
            }
        }
    }
    for (index, c) in settings.commands.iter().enumerate() {
        if let Some(s) = &c.shortcut {
            for keys in &s.keys {
                user.push(Bind {
                    keys: keys.clone(),
                    action: Box::new(RunCommand {
                        index,
                        project: None,
                    }),
                    context: s.when.clone(),
                });
            }
        }
    }
    defaults.retain(|d| {
        !user
            .iter()
            .any(|u| u.keys == d.keys && (u.context.is_none() || u.context == d.context))
    });
    defaults
        .into_iter()
        .chain(user)
        .map(|b| {
            let keys: Vec<String> = b.keys.iter().map(Keystroke::unparse).collect();
            let predicate = b
                .context
                .map(|c| Rc::new(KeyBindingContextPredicate::parse(&c).expect("checked")));
            KeyBinding::load(
                &keys.join(" "),
                b.action,
                predicate,
                false,
                None,
                &gpui::DummyKeyboardMapper,
            )
            .expect("checked")
        })
        .collect()
}

/// An action's keys with `settings`: the config file's if it sets them,
/// else the defaults (each once, though bound in several contexts). True
/// when the config file sets them.
pub fn keys_for(settings: &Settings, act: &Act) -> (Vec<Vec<Keystroke>>, bool) {
    match settings.shortcuts.get(act.name) {
        Some(s) => (s.keys.clone(), true),
        None => {
            let mut keys: Vec<Vec<Keystroke>> = vec![];
            for k in act.bindings.iter().flat_map(|(_, ks)| ks.iter()) {
                let k = parse_seq(k).expect("default shortcut parses");
                if !keys.contains(&k) {
                    keys.push(k);
                }
            }
            (keys, false)
        }
    }
}

/// A keystroke sequence as the palette and menus show it: `⌘⇧P`.
pub fn label(seq: &[Keystroke]) -> String {
    seq.iter()
        .map(|k| crate::palette::key_label(&k.modifiers, &k.key))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A key pressed in the Settings window's shortcut recorder, as the config
/// file writes it (`cmd-shift-k`), or `None` if it can't be a shortcut: it
/// needs ⌘, ⌃ or ⌥, unless it's a function key, so typing isn't taken over.
pub fn record(ks: &Keystroke) -> Option<String> {
    let m = &ks.modifiers;
    let function_key = ks
        .key
        .strip_prefix('f')
        .and_then(|n| n.parse::<u8>().ok())
        .is_some_and(|n| (1..=20).contains(&n));
    if !(m.platform || m.control || m.alt || function_key) {
        return None;
    }
    let mut k = ks.clone();
    k.key_char = None;
    k.modifiers.function = false;
    // ⌘⇧] arrives as cmd-} with shift still set; the config writes cmd-}.
    if SHIFTED.iter().any(|(_, shifted)| *shifted == k.key) {
        k.modifiers.shift = false;
    }
    let text = k.unparse();
    // Only what the config file reads back as this same key.
    (parse_seq(&text).ok()? == [k]).then_some(text)
}

/// `abromux config shortcuts`: each action's name and its keys with
/// `settings`, marking the ones the config file sets.
pub fn describe(settings: &Settings) -> String {
    let mut out = String::new();
    for act in ACTIONS {
        let (keys, set) = keys_for(settings, act);
        let keys = if keys.is_empty() {
            "-".into()
        } else {
            keys.iter().map(|k| label(k)).collect::<Vec<_>>().join(", ")
        };
        let mark = if set { "  (config)" } else { "" };
        out.push_str(&format!("{:<22}{keys}{mark}\n", act.name));
    }
    for c in &settings.commands {
        let keys = c.shortcut.as_ref().map_or("-".into(), |s| {
            s.keys
                .iter()
                .map(|k| label(k))
                .collect::<Vec<_>>()
                .join(", ")
        });
        out.push_str(&format!(
            "{:<22}{keys}  (command)\n",
            format!("{:?}", c.title)
        ));
    }
    out
}

/// Named keys gpui knows, besides single characters.
const NAMED_KEYS: &[&str] = &[
    "enter",
    "escape",
    "tab",
    "space",
    "backspace",
    "delete",
    "insert",
    "home",
    "end",
    "pageup",
    "pagedown",
    "left",
    "right",
    "up",
    "down",
];

/// A US keyboard's shifted punctuation. macOS reports ⌘⇧] as cmd-}, so a
/// shortcut written with shift and the unshifted key is folded the same way.
const SHIFTED: &[(&str, &str)] = &[
    ("`", "~"),
    ("-", "_"),
    ("=", "+"),
    ("[", "{"),
    ("]", "}"),
    ("\\", "|"),
    (";", ":"),
    ("'", "\""),
    (",", "<"),
    (".", ">"),
    ("/", "?"),
];

/// A shortcut as the config file writes it, a keystroke or a chord of them
/// separated by spaces, parsed into what macOS will report.
pub fn parse_seq(s: &str) -> Result<Vec<Keystroke>, String> {
    let keys: Vec<Keystroke> = s
        .split_whitespace()
        .map(parse_key)
        .collect::<Result<_, _>>()?;
    if keys.is_empty() {
        return Err("empty shortcut".into());
    }
    Ok(keys)
}

fn parse_key(s: &str) -> Result<Keystroke, String> {
    let bad = || format!("{s:?} isn't a keystroke; write it like \"cmd-shift-p\" or \"⌘⇧P\"");
    // ⌘⇧P → cmd-shift-p.
    let mut dashed = String::new();
    let mut rest = s;
    while let Some(c) = rest.chars().next() {
        let m = match c {
            '⌘' => "cmd",
            '⌥' => "alt",
            '⌃' => "ctrl",
            '⇧' => "shift",
            _ => break,
        };
        dashed.push_str(m);
        dashed.push('-');
        rest = &rest[c.len_utf8()..];
    }
    let rest = match rest {
        "↩" => "enter",
        "⎋" => "escape",
        "⇥" => "tab",
        "⌫" => "backspace",
        "←" => "left",
        "→" => "right",
        "↑" => "up",
        "↓" => "down",
        r => r,
    };
    // Other spellings of the modifiers.
    for part in rest.split_inclusive('-') {
        let (word, dash) = part.strip_suffix('-').map_or((part, ""), |w| (w, "-"));
        let word = match word.to_ascii_lowercase().as_str() {
            "command" => "cmd".to_string(),
            "option" | "opt" => "alt".to_string(),
            "control" => "ctrl".to_string(),
            // gpui reads a capital letter as shift plus the letter; here it's just the letter.
            w if word.len() == 1 => w.to_string(),
            _ => word.to_string(),
        };
        dashed.push_str(&word);
        dashed.push_str(dash);
    }
    let mut k = Keystroke::parse(&dashed).map_err(|_| bad())?;
    let named = NAMED_KEYS.contains(&k.key.as_str())
        || k.key
            .strip_prefix('f')
            .and_then(|n| n.parse::<u8>().ok())
            .is_some_and(|n| (1..=20).contains(&n));
    if k.key.chars().count() != 1 && !named {
        return Err(bad());
    }
    if k.modifiers.shift
        && let Some((_, shifted)) = SHIFTED.iter().find(|(plain, _)| *plain == k.key)
    {
        k.modifiers.shift = false;
        k.key = shifted.to_string();
    }
    k.key_char = None;
    Ok(k)
}

/// Checks a `when` key context, like `Terminal` or `Browser > Input`.
pub fn check_context(s: &str) -> Result<(), String> {
    KeyBindingContextPredicate::parse(s)
        .map(|_| ())
        .map_err(|e| format!("{s:?} isn't a key context: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(s: &str) -> Vec<String> {
        parse_seq(s)
            .unwrap()
            .iter()
            .map(Keystroke::unparse)
            .collect()
    }

    #[test]
    fn defaults_parse_and_names_are_unique() {
        for (i, a) in ACTIONS.iter().enumerate() {
            assert!(
                ACTIONS[..i].iter().all(|b| b.name != a.name),
                "{} twice",
                a.name
            );
            for (_, ks) in a.bindings {
                for k in *ks {
                    parse_seq(k).unwrap_or_else(|e| panic!("{}: {e}", a.name));
                }
            }
        }
    }

    #[test]
    fn spellings() {
        assert_eq!(keys("cmd-shift-p"), ["cmd-shift-p"]);
        assert_eq!(keys("⌘⇧P"), ["cmd-shift-p"]);
        assert_eq!(keys("cmd-P"), ["cmd-p"]);
        assert_eq!(keys("command-option-left"), ["alt-cmd-left"]);
        assert_eq!(keys("⌥⌘←"), ["alt-cmd-left"]);
        assert_eq!(keys("ctrl-a h"), ["ctrl-a", "h"]);
        assert_eq!(keys("cmd--"), ["cmd--"]);
        assert_eq!(keys("f12"), ["f12"]);
    }

    #[test]
    fn shift_folds_into_punctuation() {
        assert_eq!(keys("cmd-shift-]"), ["cmd-}"]);
        assert_eq!(keys("⌘⇧,"), ["cmd-<"]);
        assert_eq!(keys("cmd-shift-="), ["cmd-+"]);
        assert_eq!(
            parse_seq("cmd-}").unwrap(),
            parse_seq("cmd-shift-]").unwrap()
        );
    }

    #[test]
    fn user_keys_replace_defaults() {
        let settings: Settings = serde_json::from_str(
            r#"{
              "shortcuts": { "splitRight": "cmd-k", "newWorkspace": null },
              "commands": [{ "title": "t", "run": "ls", "shortcut": "cmd-t" }]
            }"#,
        )
        .unwrap();
        let bindings = bindings_for(&settings);
        let named = |name: &str| -> Vec<String> {
            bindings
                .iter()
                .filter(|b| b.action().name() == name)
                .map(|b| b.keystrokes().iter().map(|k| k.inner().unparse()).collect())
                .collect()
        };
        assert_eq!(named("abromux::SplitRight"), ["cmd-k"]);
        assert!(named("abromux::NewWorkspace").is_empty());
        // An app-wide key takes the default away in every context.
        assert!(named("terminal::ClearScrollback").is_empty());
        assert!(named("abromux::NewTerminal").is_empty());
        assert_eq!(named("abromux::RunCommand"), ["cmd-t"]);
        assert_eq!(named("terminal::Find").len(), 3, "one per context");
    }

    #[test]
    fn recorded_keys() {
        let rec = |s: &str| {
            let mut k = Keystroke::parse(s).unwrap();
            k.key_char = Some("x".into());
            record(&k)
        };
        assert_eq!(rec("cmd-shift-k").as_deref(), Some("cmd-shift-k"));
        assert_eq!(rec("alt-cmd-left").as_deref(), Some("alt-cmd-left"));
        assert_eq!(rec("cmd-shift-}").as_deref(), Some("cmd-}"));
        assert_eq!(rec("cmd-shift-<").as_deref(), Some("cmd-<"));
        assert_eq!(rec("f5").as_deref(), Some("f5"));
        assert_eq!(rec("k"), None, "plain keys are for typing");
        assert_eq!(rec("shift-k"), None);
    }

    #[test]
    fn keys_for_defaults_and_config() {
        let settings: Settings =
            serde_json::from_str(r#"{ "shortcuts": { "splitRight": "cmd-k" } }"#).unwrap();
        let labels = |name: &str| {
            let (keys, set) = keys_for(&settings, find(name).unwrap());
            (keys.iter().map(|k| label(k)).collect::<Vec<_>>(), set)
        };
        assert_eq!(labels("splitRight"), (vec!["⌘K".to_string()], true));
        // Bound in three contexts, listed once.
        assert_eq!(labels("find"), (vec!["⌘F".to_string()], false));
        assert_eq!(labels("openSettings"), (vec!["⌘,".to_string()], false));
    }

    #[test]
    fn bad_keys() {
        assert!(parse_seq("").is_err());
        assert!(parse_seq("cmd-foo").is_err());
        assert!(parse_seq("cmdd-p").is_err());
        assert!(check_context("Terminal").is_ok());
        assert!(check_context("Browser > Input").is_ok());
        assert!(check_context("Terminal &&").is_err());
    }
}
