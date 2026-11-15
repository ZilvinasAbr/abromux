//! Linux: freedesktop and Wayland. For now (Phase 1 of docs/linux-plan.md)
//! these are stubs with the macOS API that compile and mostly do nothing;
//! Phase 2 fills them in. Where the real thing is a one-liner (`xdg-open`,
//! `/proc`), it's here already.

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
pub mod window;
