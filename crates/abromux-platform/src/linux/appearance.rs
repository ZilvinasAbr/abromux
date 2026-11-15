//! The app's light or dark appearance.

/// What the app's windows look like.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    /// Follow the system.
    System,
    Light,
    Dark,
}

/// Nothing yet: Phase 2 reads xdg-desktop-portal's `color-scheme`.
pub fn set(_look: Look) {}
