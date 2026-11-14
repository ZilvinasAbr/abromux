//! The app's light or dark appearance.

use objc::{class, msg_send, runtime::Object, sel, sel_impl};

/// What the app's windows look like.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    /// Follow the system.
    System,
    Light,
    Dark,
}

/// Set macOS's appearance for the app (`NSApp.appearance`): its windows,
/// their blur, menus and web views follow it. System clears it, so they
/// follow macOS's.
pub fn set(look: Look) {
    #[link(name = "AppKit", kind = "framework")]
    unsafe extern "C" {
        static NSAppearanceNameAqua: *mut Object;
        static NSAppearanceNameDarkAqua: *mut Object;
    }
    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let appearance: *mut Object = match look {
            Look::System => std::ptr::null_mut(),
            Look::Light => {
                msg_send![class!(NSAppearance), appearanceNamed: NSAppearanceNameAqua]
            }
            Look::Dark => {
                msg_send![class!(NSAppearance), appearanceNamed: NSAppearanceNameDarkAqua]
            }
        };
        let _: () = msg_send![app, setAppearance: appearance];
    }
}
