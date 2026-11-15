// Leave the cursor over browser panes to WebKit.
//
// GPUI sets the cursor after every frame it draws and whenever the hovered
// hitbox changes. It knows nothing about the native WKWebView, so over a page
// it sets the arrow, while WebKit sets the hand over links on the next mouse
// move: the cursor flickers between the two. Here -[NSCursor set] ignores
// calls from our own binary (GPUI) while the mouse is over a WKWebView.

#import <AppKit/AppKit.h>
#import <WebKit/WebKit.h>
#import <dlfcn.h>
#import <mach-o/dyld.h>
#import <objc/runtime.h>

static IMP original_set;

static BOOL MouseOverWebView(void) {
    NSInteger number = [NSWindow windowNumberAtPoint:[NSEvent mouseLocation]
                         belowWindowWithWindowNumber:0];
    NSWindow *window = [NSApp windowWithWindowNumber:number];
    NSView *content = window.contentView;
    if (!content) return NO;
    NSView *frame = content.superview ?: content;
    NSPoint point = [frame convertPoint:window.mouseLocationOutsideOfEventStream fromView:nil];
    for (NSView *view = [content hitTest:point]; view; view = view.superview) {
        if ([view isKindOfClass:[WKWebView class]]) return YES;
    }
    return NO;
}

static BOOL FromMainExecutable(const void *address) {
    Dl_info info;
    return dladdr(address, &info) && info.dli_fbase == (void *)_dyld_get_image_header(0);
}

static void GuardedSet(NSCursor *self, SEL _cmd) {
    if (FromMainExecutable(__builtin_return_address(0)) && MouseOverWebView()) return;
    ((void (*)(id, SEL))original_set)(self, _cmd);
}

void abromux_cursor_install(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        Method method = class_getInstanceMethod([NSCursor class], @selector(set));
        original_set = method_setImplementation(method, (IMP)GuardedSet);
    });
}
