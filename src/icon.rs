#![allow(unexpected_cfgs)]
//! Dock icon. abromux runs as a bare binary (no .app bundle), so the icon is set
//! at runtime. Source art is assets/app/abromux.svg; abromux.png/.icns are renders.

use objc::{class, msg_send, runtime::Object, sel, sel_impl};

static PNG: &[u8] = include_bytes!("../assets/app/abromux.png");

pub fn set_dock_icon() {
    unsafe {
        let data: *mut Object =
            msg_send![class!(NSData), dataWithBytes: PNG.as_ptr() length: PNG.len()];
        let image: *mut Object = msg_send![class!(NSImage), alloc];
        let image: *mut Object = msg_send![image, initWithData: data];
        if image.is_null() {
            return;
        }
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, setApplicationIconImage: image];
    }
}
