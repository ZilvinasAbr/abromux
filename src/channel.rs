//! Stable vs nightly, and each channel's folders: the app's names for what
//! `abromux_platform::{channel, dirs}` work out. Use [`app_name`] for the
//! user-visible name and [`support_dir`] for files under Application
//! Support, never a hardcoded "AbroMux" or `abromux/` path.

pub use abromux_platform::{
    channel::{app_name, nightly},
    dirs::{support_dir, support_dir_named, workspaces_dir},
};
