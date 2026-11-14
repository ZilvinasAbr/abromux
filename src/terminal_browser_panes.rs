//! `browser.terminalBrowserPanes`: terminal-browser opens what would be a new
//! tab of its own (a `target=_blank` link, ⌘- or middle-click, "Open link in
//! new tab", its new-tab box) as an abromux split beside the browser.
//!
//! terminal-browser has no setting for that, so abromux puts a few lines into
//! its `app/browser/dist/main.js` (the JS its daemon runs) and takes them out
//! again when the setting is off. That file is outside its signed Electron
//! app, whose signature, entitlements and so passkeys stay as they are. A
//! `terminal-browser upgrade` replaces the file; [`sync`] runs before each
//! terminal-browser abromux starts, and puts them back. If the code they hook
//! into has moved, it changes nothing and says so.
//!
//! Pages opened with `window.open` and window features stay popups: OAuth and
//! SSO sign-ins talk to the page that opened them. A running daemon keeps the
//! code it started with, so a change applies once every terminal-browser has
//! quit and the next one starts it again.

use std::{
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::SystemTime,
};

/// The function the hooks call (`__abxOpenPane(url, env)`), between markers.
const HELPER: &str = include_str!("terminal_browser_panes.js");
/// The start of the helper, in any version of it.
const BEGIN: &str = "/* abromux: terminal-browser-panes v";
const END: &str = "/* abromux: terminal-browser-panes (end) */\n";
/// main.js's first line; the helper goes right after it.
const HEAD: &str = "\"use strict\";\n";

/// terminal-browser's code and what it becomes; each must be there once.
/// Each session's `ctx.env` is its own pane's.
const EDITS: &[(&str, &str)] = &[
    // TabManager.openWindow: a link that asks for a new tab.
    (
        "    this.create(details.url, details.disposition === \"foreground-tab\");\n",
        "    if (__abxOpenPane(details.url, this.__abxEnv)) return \"deny\";\n    \
         this.create(details.url, details.disposition === \"foreground-tab\");\n",
    ),
    // The page's context menu.
    (
        "      case \"open-link-tab\":\n        this.tabs.create(menu.linkURL);\n",
        "      case \"open-link-tab\":\n        \
         if (!__abxOpenPane(menu.linkURL, this.ctx.env)) this.tabs.create(menu.linkURL);\n",
    ),
    // The new-tab box.
    (
        "      if (text13.trim()) this.tabs.create(this.resolveInput(text13));\n",
        "      if (text13.trim()) { const u = this.resolveInput(text13); \
         if (!__abxOpenPane(u, this.ctx.env)) this.tabs.create(u); }\n",
    ),
    // The session hands its env to its TabManager, for openWindow.
    (
        "      this.defaultUrl\n    );\n  }\n  lastZoom = 1;",
        "      this.defaultUrl\n    );\n    this.tabs.__abxEnv = ctx.env;\n  }\n  lastZoom = 1;",
    ),
];

/// `src` (stock main.js) with the hooks in.
fn patch(src: &str) -> Result<String, String> {
    let Some(body) = src.strip_prefix(HEAD) else {
        return Err("its main.js doesn't start as expected".into());
    };
    let mut out = body.to_string();
    for (stock, ours) in EDITS {
        match out.matches(stock).count() {
            1 => out = out.replacen(stock, ours, 1),
            n => {
                return Err(format!(
                    "the code it hooks into was found {n} times, not once; \
                     this terminal-browser version isn't supported yet"
                ));
            }
        }
    }
    Ok(format!("{HEAD}{HELPER}{out}"))
}

/// `src` without the hooks, or an error if they can't all be taken out (an
/// older abromux's, which hooked in elsewhere).
fn unpatch(src: &str) -> Result<String, String> {
    let mut out = src.to_string();
    if let Some(start) = out.find(BEGIN)
        && let Some(end) = out[start..].find(END)
    {
        out.replace_range(start..start + end + END.len(), "");
    }
    for (stock, ours) in EDITS {
        out = out.replace(ours, stock);
    }
    if out.contains("__abx") {
        return Err("couldn't take out an older abromux's change; \
                    reinstall terminal-browser (`terminal-browser upgrade`)"
            .into());
    }
    Ok(out)
}

/// `src` as it should be: hooked in (this version's) if `on`, stock if not.
/// None when it already is.
fn wanted(src: &str, on: bool) -> Result<Option<String>, String> {
    let hooked = src.contains(BEGIN);
    let current = src.contains(HELPER);
    match (on, hooked) {
        (true, true) if current => Ok(None),
        (true, true) => patch(&unpatch(src)?).map(Some),
        (true, false) => patch(src).map(Some),
        (false, true) => unpatch(src).map(Some),
        (false, false) => Ok(None),
    }
}

/// The id of the toast that says the setting couldn't be applied.
pub struct Failed;

static ON: AtomicBool = AtomicBool::new(false);
/// main.js as [`sync`] last left it (path, modified time, size) and for which
/// setting, so a launch with nothing changed only looks at its metadata.
static SEEN: Mutex<Option<Seen>> = Mutex::new(None);
type Seen = (PathBuf, Option<(SystemTime, u64)>, bool);

fn stamp(p: &Path) -> Option<(SystemTime, u64)> {
    let m = std::fs::metadata(p).ok()?;
    Some((m.modified().ok()?, m.len()))
}

/// Take the setting (at start and on each change) and bring main.js in line.
pub fn set(on: bool) -> Result<(), String> {
    ON.store(on, Ordering::Relaxed);
    sync()
}

/// Bring terminal-browser's main.js in line with the setting. Cheap when
/// nothing changed since the last call; Ok if terminal-browser isn't there.
pub fn sync() -> Result<(), String> {
    let on = ON.load(Ordering::Relaxed);
    let Some(main) = main_js() else {
        return Ok(());
    };
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if seen.as_ref() == Some(&(main.clone(), stamp(&main), on)) {
        return Ok(());
    }
    let result = std::fs::read_to_string(&main)
        .map_err(|e| e.to_string())
        .and_then(|src| wanted(&src, on))
        .and_then(|new| match new {
            Some(new) => write(&main, &new),
            None => Ok(()),
        });
    // A failure isn't tried again until the file or the setting changes.
    *seen = Some((main.clone(), stamp(&main), on));
    result.map_err(|e| format!("terminal-browser new tabs as panes: {e}"))
}

/// Replace `path` whole, keeping its permissions.
fn write(path: &Path, contents: &str) -> Result<(), String> {
    let tmp = path.with_extension("js.abromux-tmp");
    let perms = std::fs::metadata(path)
        .map_err(|e| e.to_string())?
        .permissions();
    std::fs::write(&tmp, contents)
        .and_then(|()| std::fs::set_permissions(&tmp, perms))
        .and_then(|()| std::fs::rename(&tmp, path))
        .map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("couldn't write {}: {e}", path.display())
        })
}

/// terminal-browser's `app/browser/dist/main.js`: next to the `terminal-browser`
/// abromux runs (a link into its app folder, or the installer's script that
/// execs one), else where its installer puts it.
fn main_js() -> Option<PathBuf> {
    let in_root = |root: &Path| Some(root.join("browser/dist/main.js")).filter(|p| p.is_file());
    let bin = crate::terminal_browser::find();
    let from_bin = bin.as_ref().and_then(|b| {
        let real = std::fs::canonicalize(b).ok()?;
        in_root(real.parent()?.parent()?)
    });
    let from_script = || {
        let b = bin.as_ref()?;
        let text = std::fs::read(b).ok().filter(|t| t.len() < 64 * 1024)?;
        let text = String::from_utf8(text).ok()?;
        text.split('"')
            .find(|s| s.starts_with('/') && s.ends_with("/bin/terminal-browser"))
            .and_then(|s| in_root(Path::new(s).parent()?.parent()?))
    };
    from_bin
        .or_else(from_script)
        .or_else(|| in_root(&crate::paths::home().join(".local/share/terminal-browser/app")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bits of terminal-browser 0.15.3's main.js the hooks go into.
    const STOCK: &str = "\"use strict\";\nvar a = 1;\n  openWindow(id2, details) {\n    \
        this.create(details.url, details.disposition === \"foreground-tab\");\n    \
        return \"deny\";\n  }\n      case \"open-link-tab\":\n        this.tabs.create(menu.linkURL);\n        \
        return;\n      if (text13.trim()) this.tabs.create(this.resolveInput(text13));\n    \
        this.tabs = new TabManager(\n      {},\n      this.defaultUrl\n    );\n  }\n  lastZoom = 1;\n";

    #[test]
    fn hooks_in_and_out_again() {
        let ours = patch(STOCK).unwrap();
        assert!(ours.starts_with(&format!("{HEAD}{HELPER}")));
        assert_eq!(
            ours.matches("__abxOpenPane(").count(),
            4,
            "the helper and 3 calls"
        );
        assert!(ours.contains("this.tabs.__abxEnv = ctx.env;"));
        assert_eq!(unpatch(&ours).unwrap(), STOCK);
    }

    #[test]
    fn follows_the_setting() {
        let ours = patch(STOCK).unwrap();
        assert_eq!(wanted(STOCK, true).unwrap().as_deref(), Some(&*ours));
        assert_eq!(wanted(&ours, true).unwrap(), None, "already in");
        assert_eq!(wanted(&ours, false).unwrap().as_deref(), Some(STOCK));
        assert_eq!(wanted(STOCK, false).unwrap(), None, "nothing to take out");
        // An older helper is swapped for this one.
        let old = ours.replace(" v1 (begin)", " v0 (begin)");
        assert_eq!(wanted(&old, true).unwrap().as_deref(), Some(&*ours));
    }

    #[test]
    fn leaves_an_unknown_version_alone() {
        let moved = STOCK.replace("open-link-tab", "open-link-in-tab");
        assert!(patch(&moved).is_err());
        let twice = STOCK.replace(
            "  lastZoom = 1;",
            "  lastZoom = 1;\n      this.defaultUrl\n    );\n  }\n  lastZoom = 1;",
        );
        assert!(patch(&twice).is_err(), "a hook point twice");
        assert!(patch("var a;").is_err(), "no \"use strict\" first");
        let stray = format!("{STOCK}__abxOpenPane(1)");
        assert!(unpatch(&stray).is_err());
    }

    #[test]
    fn helper_is_marked() {
        assert!(HELPER.starts_with(BEGIN));
        assert!(HELPER.ends_with(END));
    }
}
