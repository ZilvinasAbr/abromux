// What the Chromium engine (crates/abromux-browser/src/backend/chromium.rs)
// needs from the app object. Chromium's event handling on macOS asks NSApp
// whether it's inside -sendEvent: (CrAppProtocol / CrAppControlProtocol /
// CefAppProtocol); a CEF app normally subclasses NSApplication for it. gpui
// owns NSApp's class, so the methods are added to that class at runtime,
// before CEF starts, and -sendEvent: is wrapped to keep the flag.

#import <AppKit/AppKit.h>
#import <objc/runtime.h>

static BOOL gHandlingSendEvent;

static Protocol *EnsureProtocol(const char *name, Protocol *parent) {
    Protocol *p = objc_getProtocol(name);
    if (p) return p;
    p = objc_allocateProtocol(name);
    if (!p) return objc_getProtocol(name);
    if (parent) protocol_addProtocol(p, parent);
    objc_registerProtocol(p);
    return p;
}

/// Make NSApp a CefAppProtocol app. Idempotent; main thread.
void abx_chromium_app_install(void) {
    static BOOL done;
    if (done) return;
    done = YES;
    Class cls = object_getClass(NSApp);
    if (!cls) return;

    IMP isHandling = imp_implementationWithBlock(^BOOL(id me) {
      (void)me;
      return gHandlingSendEvent;
    });
    IMP setHandling = imp_implementationWithBlock(^(id me, BOOL v) {
      (void)me;
      gHandlingSendEvent = v;
    });
    class_addMethod(cls, @selector(isHandlingSendEvent), isHandling, "c@:");
    class_addMethod(cls, @selector(setHandlingSendEvent:), setHandling, "v@:c");

    SEL send = @selector(sendEvent:);
    Method m = class_getInstanceMethod(cls, send);
    void (*orig)(id, SEL, NSEvent *) = (void *)method_getImplementation(m);
    IMP wrapped = imp_implementationWithBlock(^(id me, NSEvent *e) {
      BOOL was = gHandlingSendEvent;
      gHandlingSendEvent = YES;
      orig(me, send, e);
      gHandlingSendEvent = was;
    });
    // Added to gpui's own class, so NSApplication itself stays as it was;
    // replaced if that class has its own -sendEvent:.
    if (!class_addMethod(cls, send, wrapped, method_getTypeEncoding(m)))
        method_setImplementation(class_getInstanceMethod(cls, send), wrapped);

    Protocol *cr = EnsureProtocol("CrAppProtocol", nil);
    Protocol *control = EnsureProtocol("CrAppControlProtocol", cr);
    Protocol *cef = EnsureProtocol("CefAppProtocol", control);
    for (Protocol *p in @[ cr, control, cef ]) {
        if (p) class_addProtocol(cls, p);
    }
}

/// Place `view` (a page) at `x, y, w, h` in its superview, in points from
/// the superview's top left whether or not it's flipped; and show or hide it.
void abx_chromium_view_place(void *view, double x, double y, double w, double h, bool shown) {
    NSView *v = (__bridge NSView *)view;
    NSView *parent = v.superview;
    if (!parent) return;
    CGFloat top = parent.isFlipped ? y : parent.bounds.size.height - y - h;
    NSRect frame = NSMakeRect(x, top, MAX(w, 0), MAX(h, 0));
    if (!NSEqualRects(v.frame, frame)) v.frame = frame;
    if (v.hidden == shown) v.hidden = !shown;
}

/// Whether the key window's first responder is `view` or inside it.
bool abx_chromium_view_has_focus(void *view) {
    NSView *v = (__bridge NSView *)view;
    NSResponder *r = v.window.firstResponder;
    return [r isKindOfClass:NSView.class] && [(NSView *)r isDescendantOf:v];
}

/// Give keyboard focus back to the window's content (gpui's view).
void abx_chromium_view_unfocus(void *view) {
    NSView *v = (__bridge NSView *)view;
    NSWindow *w = v.window;
    if (w && abx_chromium_view_has_focus(view)) [w makeFirstResponder:v.superview];
}

/// Take `view` out of the window (its page is closing).
void abx_chromium_view_remove(void *view) {
    [(__bridge NSView *)view removeFromSuperview];
}
