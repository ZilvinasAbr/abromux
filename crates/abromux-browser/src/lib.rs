//! Browser panes' parts that aren't the pane's view: the page itself, behind
//! [`backend::PageBackend`], and the history, find, automation and
//! downloads that work on it. The pane (address bar, toolbar, find bar) is
//! `src/browser.rs` in the app. See docs/linux-plan.md.

pub mod auto;
pub mod backend;
pub mod downloads;
pub mod find;
pub mod history;
