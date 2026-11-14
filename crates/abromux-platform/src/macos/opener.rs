//! Hand files and URLs to the apps the user picked for them.

use std::{ffi::OsStr, io, process::Command};

/// Open `target` (a file, folder or URL) in its default app, without waiting.
pub fn open(target: impl AsRef<OsStr>) -> io::Result<()> {
    Command::new("open").arg(target).spawn().map(drop)
}

/// Show `path` selected in Finder, without waiting.
pub fn reveal(path: impl AsRef<OsStr>) -> io::Result<()> {
    Command::new("open").arg("-R").arg(path).spawn().map(drop)
}
