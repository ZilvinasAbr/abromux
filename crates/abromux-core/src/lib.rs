//! abromux's logic that needs no UI and no OS calls: protocols, parsers and
//! models, unit-tested on every OS. Depends on nothing else of abromux's.
//! See docs/linux-plan.md.

pub mod agent;
pub mod ipc;
pub mod link;
pub mod naming;
pub mod notify_hook;
pub mod osc99;
pub mod paths;
pub mod settings;
pub mod status;
pub mod text;
