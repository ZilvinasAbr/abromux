//! The app icon. Source art is assets/app/abromux.svg (abromux-nightly.svg
//! for the nightly app); the .png/.icns files are renders.

static PNG: &[u8] = include_bytes!("../assets/app/abromux.png");
static NIGHTLY_PNG: &[u8] = include_bytes!("../assets/app/abromux-nightly.png");

/// This channel's icon art.
pub fn png() -> &'static [u8] {
    if crate::channel::nightly() {
        NIGHTLY_PNG
    } else {
        PNG
    }
}

/// Show this channel's icon in the dock: abromux can run as a bare binary
/// (no .app bundle), so it's set at runtime.
pub fn set_dock_icon() {
    abromux_platform::dock::set_icon(png());
}
