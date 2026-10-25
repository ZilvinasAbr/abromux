#![allow(unexpected_cfgs)]
//! Dock icon. abromux runs as a bare binary (no .app bundle), so the icon is set
//! at runtime. Source art is assets/app/abromux.svg (abromux-nightly.svg for the nightly
//! app); the .png/.icns files are renders.

use objc::{class, msg_send, runtime::Object, sel, sel_impl};

static PNG: &[u8] = include_bytes!("../assets/app/abromux.png");
static NIGHTLY_PNG: &[u8] = include_bytes!("../assets/app/abromux-nightly.png");

pub fn set_dock_icon() {
    let png = if crate::channel::nightly() {
        NIGHTLY_PNG
    } else {
        PNG
    };
    unsafe {
        let data: *mut Object =
            msg_send![class!(NSData), dataWithBytes: png.as_ptr() length: png.len()];
        let image: *mut Object = msg_send![class!(NSImage), alloc];
        let image: *mut Object = msg_send![image, initWithData: data];
        if image.is_null() {
            return;
        }
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, setApplicationIconImage: image];
    }
}
