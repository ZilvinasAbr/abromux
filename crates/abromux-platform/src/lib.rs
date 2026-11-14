//! Everything abromux asks of the operating system, behind one API, so the
//! rest of the app has no OS calls of its own. Only macOS for now; see
//! docs/linux-plan.md.

mod macos;

pub use macos::{browser_native, cursor, webauthn};
