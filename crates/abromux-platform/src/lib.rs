//! Everything abromux asks of the operating system, behind one API, so the
//! rest of the app has no OS calls of its own. Only macOS for now; see
//! docs/linux-plan.md.
//!
//! - [`channel`] and [`dirs`]: stable or nightly, and where files go.
//! - [`notify`]: banners and the dock badge; [`tray`]: the menu-bar bell.
//! - [`power`]: keeping the app awake while terminals print (App Nap).
//! - [`appearance`], [`dock`], [`window`]: light or dark, the app icon,
//!   titlebar drags and the debug snapshots' synthetic input.
//! - [`prefs`]: small preferences, such as the "Don't ask again" boxes.
//! - [`opener`]: files and URLs to their default apps; [`process`]: other
//!   processes' working folders.
//! - [`browser_native`], [`cursor`], [`webauthn`]: the browser pane's native
//!   parts.

pub mod channel;
pub mod dirs;
mod macos;
pub mod prefs;

pub use macos::{
    appearance, browser_native, cursor, dock, notify, opener, power, process, tray, webauthn,
    window,
};
