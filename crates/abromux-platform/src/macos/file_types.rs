//! Which app opens a kind of file: Launch Services and Uniform Type
//! Identifiers, through CoreServices.

use std::ffi::c_void;

type CFStringRef = *const c_void;
const UTF8: u32 = 0x0800_0100;
/// `kLSRolesAll`: viewer and editor.
const ROLES_ALL: u32 = 0xFFFF_FFFF;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithBytes(
        alloc: *const c_void,
        bytes: *const u8,
        len: isize,
        encoding: u32,
        external: u8,
    ) -> CFStringRef;
    // As `super::window` declares it: one name, one signature.
    fn CFRelease(cf: *mut objc::runtime::Object);
}

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    static kUTTagClassFilenameExtension: CFStringRef;
    fn UTTypeCreatePreferredIdentifierForTag(
        class: CFStringRef,
        tag: CFStringRef,
        conforming_to: CFStringRef,
    ) -> CFStringRef;
    fn LSSetDefaultRoleHandlerForContentType(
        content_type: CFStringRef,
        role: u32,
        handler: CFStringRef,
    ) -> i32;
}

/// An owned CFString, released when dropped.
struct CfString(CFStringRef);

impl CfString {
    fn new(s: &str) -> Self {
        Self(unsafe {
            CFStringCreateWithBytes(std::ptr::null(), s.as_ptr(), s.len() as isize, UTF8, 0)
        })
    }
}

impl Drop for CfString {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0 as *mut _) }
        }
    }
}

/// The type files ending in `.ext` have (a `dyn.…` one if macOS knows
/// none), with `bundle_id` as its handler. An OSStatus on failure.
pub fn set_default(bundle_id: &str, ext: &str) -> Result<(), i32> {
    let (tag, handler) = (CfString::new(ext), CfString::new(bundle_id));
    // Conforming to public.data, as files are: Launch Services refuses
    // (-50) the `dyn.` type made with no conformance, for .rs or .toml.
    let data = CfString::new("public.data");
    let uti = CfString(unsafe {
        UTTypeCreatePreferredIdentifierForTag(kUTTagClassFilenameExtension, tag.0, data.0)
    });
    if uti.0.is_null() {
        return Err(-50);
    }
    match unsafe { LSSetDefaultRoleHandlerForContentType(uti.0, ROLES_ALL, handler.0) } {
        0 => Ok(()),
        e => Err(e),
    }
}
