//! Files abromux is asked to open: from Finder (Open With, or a file type
//! whose default app is abromux; the app's `CFBundleDocumentTypes` are in
//! `scripts/bundle-app.sh`), `abromux edit`, or a restored session. Each
//! opens in a terminal running the editor `editor.program` names, in a new
//! pane beside the focused one. Quitting the editor closes the pane.
//!
//! `abromux default-app` makes abromux the default app for file
//! extensions (`set_default_app`), so a double-click opens them here.

use crate::{
    AbroMux, TerminalOpts,
    settings::{EditorProgram, Settings},
    terminal::TerminalApp,
    terminal_browser::shell_quote,
    workspace::Surface,
};
use gpui::{App, Axis, Context, Window};
use gpui_component::{WindowExt, notification::Notification};
use std::path::{Path, PathBuf};

/// The tab of a terminal editing a file.
pub struct Tab {
    file: PathBuf,
}

impl TerminalApp for Tab {
    fn icon(&self) -> &'static str {
        "pencil"
    }

    fn title(&self) -> String {
        self.file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.file.to_string_lossy().into_owned())
    }

    fn edits(&self) -> Option<&Path> {
        Some(&self.file)
    }
}

/// The shell command that edits `file` with `program`, or None if it isn't
/// installed. A bare `tode <file>` hands the file to a tode window that's
/// already open and exits, so `-n` keeps it in this pane.
pub fn command(program: EditorProgram, file: &Path) -> Option<String> {
    let bin = crate::paths::find_program(program.key())?;
    let flags = match program {
        EditorProgram::Nvim => "",
        EditorProgram::Tode => " -n",
    };
    Some(format!(
        "exec {}{flags} {}",
        shell_quote(&bin.to_string_lossy()),
        shell_quote(&file.to_string_lossy())
    ))
}

/// The local paths among the URLs macOS asked the app to open.
pub fn paths(urls: &[String]) -> Vec<PathBuf> {
    urls.iter()
        .filter_map(|u| crate::link::file_url_path(u))
        .collect()
}

/// How deep `dir` is, if `file` is in it. The home folder and `/` hold
/// everything, so they say nothing about where a file belongs.
fn depth_in(file: &Path, dir: &Path) -> Option<usize> {
    let shallow = dir == Path::new("/") || dir == crate::paths::home();
    (!shallow && file.starts_with(dir)).then(|| dir.components().count())
}

impl AbroMux {
    /// A terminal tab running the editor on `file`, in `cwd`.
    pub(crate) fn make_editor(
        &mut self,
        ws: usize,
        file: &Path,
        cwd: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Surface, String> {
        let program = cx.global::<Settings>().editor.program;
        let command = command(program, file).ok_or_else(|| {
            format!(
                "`editor.program` is {0}, but {0} isn't on PATH or in ~/.local/bin, \
                 /opt/homebrew/bin or /usr/local/bin.",
                program.key()
            )
        })?;
        let opts = TerminalOpts {
            cwd,
            command: Some(command),
            ..Default::default()
        };
        let s = self.make_terminal(ws, opts, window, cx);
        if let Surface::Terminal(t) = &s {
            t.update(cx, |t, _| {
                t.app = Some(Box::new(Tab { file: file.into() }));
            });
        }
        Ok(s)
    }

    /// Open what macOS handed the app (a file opened from Finder): files
    /// in the editor, each in the workspace it belongs to, and folders as
    /// workspaces.
    pub(crate) fn open_paths(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for path in paths {
            if path.is_dir() {
                self.create_workspace(Some(path.to_string_lossy().into_owned()), true, window, cx);
                continue;
            }
            let wi = self.workspace_for(&path, cx);
            let pane = self.workspaces[wi].focused_pane;
            if let Err(e) = self.edit_file(wi, pane, &path, true, window, cx) {
                window.push_notification(Notification::warning(e).title("Can't open the file"), cx);
            }
        }
        cx.activate(true);
        window.activate_window();
    }

    /// The workspace a file belongs to: the one whose folder, or focused
    /// terminal's directory, holds it most closely. Else the selected one.
    fn workspace_for(&self, file: &Path, cx: &App) -> usize {
        let mut best = (self.active, 0);
        for (i, w) in self.workspaces.iter().enumerate() {
            let dirs = w
                .home
                .clone()
                .into_iter()
                .chain(w.cwd(cx).map(PathBuf::from));
            for d in dirs.filter_map(|d| depth_in(file, &d)) {
                if d > best.1 || (d == best.1 && i == self.active) {
                    best = (i, d);
                }
            }
        }
        best.0
    }

    /// Edit `file` in a new pane right of pane `pane` of workspace `wi`,
    /// which `focus` selects. The new tab and its pane's id.
    pub(crate) fn edit_file(
        &mut self,
        wi: usize,
        pane: usize,
        file: &Path,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(Surface, usize), String> {
        let ws = &self.workspaces[wi];
        // The workspace's folder when the file is in it, so the editor's
        // project (a tode window, nvim's :pwd) is the workspace's.
        let cwd = ws
            .home
            .as_deref()
            .filter(|h| file.starts_with(h))
            .or(file.parent())
            .map(|d| d.to_string_lossy().into_owned());
        let s = self.make_editor(ws.id, file, cwd, window, cx)?;
        if focus && wi != self.active {
            self.select_workspace(wi, window, cx);
        }
        let new = self
            .split_pane(
                wi,
                pane,
                Axis::Horizontal,
                false,
                s.clone(),
                focus,
                window,
                cx,
            )
            .ok_or("pane not found")?;
        Ok((s, new))
    }
}

/// Make the app this binary is in the default for files ending in each of
/// `extensions` (without the dot), for opening and editing. macOS asks
/// nothing: it's what Finder's "Change All…" does, one type at a time.
pub fn set_default_app(bundle_id: &str, extensions: &[String]) -> Result<(), String> {
    let mut failed = Vec::new();
    for ext in extensions {
        if let Err(code) = ls::set_default(bundle_id, ext) {
            failed.push(format!(".{ext} (error {code})"));
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "couldn't set the default app for {}",
            failed.join(", ")
        ))
    }
}

/// Extensions of source and text files that `abromux default-app code`
/// sets in one go.
pub const CODE_EXTENSIONS: &[&str] = &[
    "c",
    "cc",
    "cpp",
    "cxx",
    "h",
    "hh",
    "hpp",
    "m",
    "mm",
    "swift",
    "rs",
    "go",
    "zig",
    "java",
    "kt",
    "kts",
    "scala",
    "groovy",
    "gradle",
    "cs",
    "fs",
    "py",
    "rb",
    "php",
    "pl",
    "lua",
    "r",
    "jl",
    "ex",
    "exs",
    "erl",
    "hs",
    "ml",
    "clj",
    "dart",
    "js",
    "mjs",
    "cjs",
    "jsx",
    "ts",
    "mts",
    "cts",
    "tsx",
    "vue",
    "svelte",
    "astro",
    "css",
    "scss",
    "sass",
    "less",
    "sh",
    "bash",
    "zsh",
    "fish",
    "json",
    "jsonc",
    "json5",
    "yaml",
    "yml",
    "toml",
    "ini",
    "cfg",
    "conf",
    "env",
    "xml",
    "sql",
    "graphql",
    "gql",
    "proto",
    "tf",
    "hcl",
    "nix",
    "vim",
    "txt",
    "log",
    "csv",
    "diff",
    "patch",
    "lock",
    "mk",
    "cmake",
    "dockerfile",
];

/// Launch Services and Uniform Type Identifiers, through CoreServices.
mod ls {
    use std::ffi::c_void;

    type CFStringRef = *const c_void;
    const UTF8: u32 = 0x0800_0100;
    /// `kLSRolesAll`: viewer and editor.
    const ROLES_ALL: u32 = 0xFFFF_FFFF;

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithBytes(
            alloc: *const c_void,
            bytes: *const u8,
            len: isize,
            encoding: u32,
            external: u8,
        ) -> CFStringRef;
        // As `crate::debug` declares it: one name, one signature.
        fn CFRelease(cf: *mut objc::runtime::Object);
    }

    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        static kUTTagClassFilenameExtension: CFStringRef;
        fn UTTypeCreatePreferredIdentifierForTag(
            class: CFStringRef,
            tag: CFStringRef,
            conforming_to: CFStringRef,
        ) -> CFStringRef;
        fn LSSetDefaultRoleHandlerForContentType(
            content_type: CFStringRef,
            role: u32,
            handler: CFStringRef,
        ) -> i32;
    }

    /// An owned CFString, released when dropped.
    struct CfString(CFStringRef);

    impl CfString {
        fn new(s: &str) -> Self {
            Self(unsafe {
                CFStringCreateWithBytes(std::ptr::null(), s.as_ptr(), s.len() as isize, UTF8, 0)
            })
        }
    }

    impl Drop for CfString {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CFRelease(self.0 as *mut _) }
            }
        }
    }

    /// The type files ending in `.ext` have (a `dyn.…` one if macOS knows
    /// none), with `bundle_id` as its handler. An OSStatus on failure.
    pub fn set_default(bundle_id: &str, ext: &str) -> Result<(), i32> {
        let (tag, handler) = (CfString::new(ext), CfString::new(bundle_id));
        // Conforming to public.data, as files are: Launch Services refuses
        // (-50) the `dyn.` type made with no conformance, for .rs or .toml.
        let data = CfString::new("public.data");
        let uti = CfString(unsafe {
            UTTypeCreatePreferredIdentifierForTag(kUTTagClassFilenameExtension, tag.0, data.0)
        });
        if uti.0.is_null() {
            return Err(-50);
        }
        match unsafe { LSSetDefaultRoleHandlerForContentType(uti.0, ROLES_ALL, handler.0) } {
            0 => Ok(()),
            e => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_urls_become_paths() {
        let urls = [
            "file:///Users/me/My%20Code/main.rs".to_string(),
            "https://example.com/x.rs".to_string(),
        ];
        assert_eq!(paths(&urls), [PathBuf::from("/Users/me/My Code/main.rs")]);
    }

    #[test]
    fn depth_skips_home_and_root() {
        let home = crate::paths::home();
        let file = home.join("code/app/src/main.rs");
        assert_eq!(depth_in(&file, &home), None);
        assert_eq!(depth_in(&file, Path::new("/")), None);
        assert_eq!(depth_in(&file, &home.join("other")), None);
        let deeper = depth_in(&file, &home.join("code/app")).unwrap();
        assert!(deeper > depth_in(&file, &home.join("code")).unwrap());
    }
}
