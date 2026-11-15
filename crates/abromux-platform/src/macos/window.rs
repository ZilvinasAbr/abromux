//! Things done to the app's own windows that GPUI has no call for: moving
//! one by its titlebar, and the window number and synthetic input behind
//! the debug snapshots and `debug.*` RPCs.

use objc::{class, msg_send, runtime::Object, sel, sel_impl};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// Why a window call didn't happen.
pub type Error = &'static str;

fn ns_view(window: &impl HasWindowHandle) -> Option<*mut Object> {
    let RawWindowHandle::AppKit(h) = window.window_handle().ok()?.as_raw() else {
        return None;
    };
    Some(h.ns_view.as_ptr() as *mut Object)
}

fn ns_window(window: &impl HasWindowHandle) -> Result<*mut Object, Error> {
    let view = ns_view(window).ok_or("not an AppKit window")?;
    Ok(unsafe { msg_send![view, window] })
}

/// Start moving `window` with the mouse, from the mouse down being handled
/// now (on an empty titlebar area).
pub fn begin_drag(window: &impl HasWindowHandle) {
    let Some(view) = ns_view(window) else { return };
    unsafe {
        let win: *mut Object = msg_send![view, window];
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let event: *mut Object = msg_send![app, currentEvent];
        if !win.is_null() && !event.is_null() {
            let _: () = msg_send![win, performWindowDragWithEvent: event];
        }
    }
}

/// The window server's number for `window`, as `screencapture -l` takes it.
pub fn number(window: &impl HasWindowHandle) -> Result<isize, Error> {
    let win = ns_window(window)?;
    Ok(unsafe { msg_send![win, windowNumber] })
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Point {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Rect {
    origin: Point,
    size: Point,
}

/// A synthetic mouse event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mouse {
    Down,
    Up,
    Move,
    Drag,
}

/// Queue a native mouse event at window point `x`,`y` (from the top left)
/// to `window`, which AppKit then delivers like a real one.
pub fn post_mouse(
    window: &impl HasWindowHandle,
    mouse: Mouse,
    x: f64,
    y: f64,
    right: bool,
    clicks: isize,
) -> Result<(), Error> {
    // NSEventType: left down/up 1/2, right down/up 3/4, moved 5, left dragged 6.
    let kind: usize = match (mouse, right) {
        (Mouse::Down, false) => 1,
        (Mouse::Up, false) => 2,
        (Mouse::Down, true) => 3,
        (Mouse::Up, true) => 4,
        (Mouse::Move, _) => 5,
        (Mouse::Drag, _) => 6,
    };
    let win = ns_window(window)?;
    unsafe {
        let number: isize = msg_send![win, windowNumber];
        let content: *mut Object = msg_send![win, contentView];
        let frame: Rect = msg_send![content, frame];
        // AppKit's window coordinates start at the bottom left.
        let at = Point {
            x,
            y: frame.size.y - y,
        };
        let nil: *mut Object = std::ptr::null_mut();
        let ev: *mut Object = msg_send![class!(NSEvent),
            mouseEventWithType: kind
            location: at
            modifierFlags: 0usize
            timestamp: 0f64
            windowNumber: number
            context: nil
            eventNumber: 0isize
            clickCount: clicks
            pressure: 1f32];
        if ev.is_null() {
            return Err("NSEvent refused it");
        }
        // Such an event is always button 0, which gpui reads as left; set
        // the button on its CGEvent and make a new NSEvent from that.
        let ev = if right {
            #[link(name = "CoreGraphics", kind = "framework")]
            unsafe extern "C" {
                fn CGEventSetIntegerValueField(event: *mut Object, field: u32, value: i64);
            }
            const BUTTON_NUMBER: u32 = 3;
            let cg: *mut Object = msg_send![ev, CGEvent];
            CGEventSetIntegerValueField(cg, BUTTON_NUMBER, 1);
            msg_send![class!(NSEvent), eventWithCGEvent: cg]
        } else {
            ev
        };
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, postEvent: ev atStart: false];
    }
    Ok(())
}

/// A native scroll wheel event of `dy` pixels (positive scrolls up) at
/// window point `x`,`y`, through gpui's hit testing to whatever scrolls there.
pub fn post_scroll(window: &impl HasWindowHandle, x: f64, y: f64, dy: f64) -> Result<(), Error> {
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventCreateScrollWheelEvent(
            source: *mut Object,
            units: u32,
            wheel_count: u32,
            wheel1: i32,
            ...
        ) -> *mut Object;
        fn CGEventSetLocation(event: *mut Object, at: Point);
        fn CFRelease(cf: *mut Object);
    }
    let view = ns_view(window).ok_or("not an AppKit window")?;
    unsafe {
        let win: *mut Object = msg_send![view, window];
        let content: *mut Object = msg_send![win, contentView];
        let frame: Rect = msg_send![content, frame];
        // An event made from a CGEvent has no window, so AppKit won't route
        // it; it goes straight to gpui's view instead. Without a window its
        // locationInWindow is its screen point (bottom left), so place it
        // where that equals the window point: CoreGraphics counts from the
        // main screen's top left.
        let screens: *mut Object = msg_send![class!(NSScreen), screens];
        let main: *mut Object = msg_send![screens, objectAtIndex: 0usize];
        let main_frame: Rect = msg_send![main, frame];
        let at = Point {
            x,
            y: main_frame.size.y - (frame.size.y - y),
        };
        const PIXEL: u32 = 0;
        let cg = CGEventCreateScrollWheelEvent(std::ptr::null_mut(), PIXEL, 1, dy as i32);
        if cg.is_null() {
            return Err("CoreGraphics refused the scroll event");
        }
        CGEventSetLocation(cg, at);
        let ev: *mut Object = msg_send![class!(NSEvent), eventWithCGEvent: cg];
        CFRelease(cg);
        if ev.is_null() {
            return Err("NSEvent refused the scroll event");
        }
        // On the next run loop pass: gpui is mid-update now and the view's
        // handler would re-enter it.
        let _: () =
            msg_send![view, performSelector: sel!(scrollWheel:) withObject: ev afterDelay: 0f64];
    }
    Ok(())
}

/// Bring the app's window forward: nothing to add on macOS, where
/// activating the app does it.
pub fn raise() {}
