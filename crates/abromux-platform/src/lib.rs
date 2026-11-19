//! Everything abromux asks of the operating system, behind one API, so the
//! rest of the app has no OS calls of its own: `macos/`, and `linux/`, which
//! is mostly stubs for now (see docs/linux-plan.md).
//!
//! - [`channel`] and [`dirs`]: stable or nightly, and where files go.
//! - [`notify`]: banners and the dock badge; [`tray`]: the menu-bar bell.
//! - [`power`]: keeping the app awake while terminals print (App Nap).
//! - [`fonts`]: the default terminal and UI fonts.
//! - [`appearance`], [`dock`], [`window`]: light or dark, the app icon,
//!   titlebar drags and the debug snapshots' synthetic input.
//! - [`primary`]: the primary selection (middle-click paste) on Linux.
//! - [`files`]: the open-files limit, raised at launch so tabs don't run out.
//! - [`prefs`]: small preferences, such as the "Don't ask again" boxes.
//! - [`opener`]: files and URLs to their default apps; [`process`]: other
//!   processes' working folders.
//! - [`browser_native`], [`cursor`], [`webauthn`]: the browser pane's native
//!   parts.

pub mod channel;

/// Whether this is the macOS build: for the few choices the app makes by OS
/// that need no OS call, such as default shortcuts and how keys are written.
pub const MACOS: bool = cfg!(target_os = "macos");

pub mod dirs;
pub mod files;
pub mod prefs;
pub mod primary;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as os;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as os;

/// The WKWebView passkey bridge; Linux has no built-in browser.
#[cfg(target_os = "macos")]
pub use os::webauthn;
pub use os::{
    appearance, browser_native, cursor, dock, file_types, fonts, notify, opener, power, process,
    tray, window,
};
