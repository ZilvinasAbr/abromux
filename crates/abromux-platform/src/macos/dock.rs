//! The dock icon. abromux can run as a bare binary (no .app bundle), so the
//! icon is set at runtime.

use objc::{class, msg_send, runtime::Object, sel, sel_impl};

/// Show `png` as the app's dock icon. Bad image data leaves it as it is.
pub fn set_icon(png: &[u8]) {
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
