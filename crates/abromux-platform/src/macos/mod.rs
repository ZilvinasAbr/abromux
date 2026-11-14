//! macOS: AppKit, WebKit and friends, partly through the `.m` files next to
//! this one (built by this crate's build.rs).
// objc's msg_send! checks a cfg that rustc doesn't know.
#![allow(unexpected_cfgs)]

pub mod appearance;
pub mod browser_native;
pub mod cursor;
pub(crate) mod defaults;
pub mod dock;
pub mod file_types;
pub mod notify;
pub mod opener;
pub mod power;
pub mod process;
pub mod tray;
pub mod webauthn;
pub mod window;
