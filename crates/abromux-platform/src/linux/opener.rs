//! Hand files and URLs to the apps the user picked for them, via `xdg-open`.

use std::{
    ffi::OsStr,
    io,
    path::Path,
    process::{Command, ExitStatus},
};

/// Open `target` (a file, folder or URL) in its default app, without waiting.
pub fn open(target: impl AsRef<OsStr>) -> io::Result<()> {
    Command::new("xdg-open").arg(target).spawn().map(drop)
}

/// Open `target` in its default app and wait for `xdg-open`; its status says
/// whether it found an app for it.
pub fn open_and_wait(target: impl AsRef<OsStr>) -> io::Result<ExitStatus> {
    Command::new("xdg-open").arg(target).status()
}

/// Open the folder `path` is in, without waiting. (Selecting the file needs
/// the FileManager1 D-Bus interface; Phase 2.)
pub fn reveal(path: impl AsRef<OsStr>) -> io::Result<()> {
    let path = Path::new(path.as_ref());
    open(path.parent().unwrap_or(path))
}
