// Native parts of browser panes that wry doesn't offer (see browser_native.rs):
//
// - a script message handler in every frame (`abromuxPage`, fed by
//   browser_page.js) that keeps each frame's WKFrameInfo, so scripts can run
//   in an iframe, even a cross-origin one, and passes the page's events on
// - cookies, through the webview's WKHTTPCookieStore (HttpOnly ones too)
// - the URL of the link under the pointer, as a label at the page's corner
// - a bitmap of the page as shown, to stand in for it under popovers
// - downloads: the WKDownload behind each one (for its progress), and
//   `Content-Disposition: attachment` responses saved instead of shown

#import <AppKit/AppKit.h>
#import <WebKit/WebKit.h>
#import <objc/runtime.h>

typedef void (*ABXMessageFn)(void *ctx, const char *json);
typedef void (*ABXFreeFn)(void *ctx);
typedef void (*ABXDoneFn)(void *ctx, const char *json, const char *error);
typedef void (*ABXImageFn)(void *ctx, const uint8_t *bgra, uint32_t width, uint32_t height);

static NSString *const kHandler = @"abromuxPage";
static char kBridgeKey;

static NSString *JSON(id obj) {
    if (!obj || obj == NSNull.null) return @"null";
    if (![NSJSONSerialization isValidJSONObject:@[ obj ]]) return @"null";
    NSData *d = [NSJSONSerialization dataWithJSONObject:obj options:NSJSONWritingFragmentsAllowed error:nil];
    return d ? [[NSString alloc] initWithData:d encoding:NSUTF8StringEncoding] : @"null";
}

static id Parse(const char *json) {
    if (!json) return nil;
    NSData *d = [NSData dataWithBytes:json length:strlen(json)];
    return [NSJSONSerialization JSONObjectWithData:d options:NSJSONReadingFragmentsAllowed error:nil];
}

/// A view the mouse goes through, for the hover label.
@interface ABXPassthroughView : NSView
@end
@implementation ABXPassthroughView
- (NSView *)hitTest:(NSPoint)point {
    return nil;
}
@end

@interface ABXPage : NSObject <WKScriptMessageHandler>
@property(weak) WKWebView *webView;
@property void *ctx;
@property ABXMessageFn message;
@property ABXFreeFn free;
/// Each frame's latest WKFrameInfo, by the token its page script made.
@property(strong) NSMutableDictionary<NSString *, WKFrameInfo *> *frames;
@property(strong) ABXPassthroughView *hoverBox;
@property(strong) NSTextField *hoverLabel;
@end

@implementation ABXPage

- (void)userContentController:(WKUserContentController *)ucc
      didReceiveScriptMessage:(WKScriptMessage *)message {
    if (![message.body isKindOfClass:NSDictionary.class]) return;
    NSMutableDictionary *body = [message.body mutableCopy];
    NSString *kind = body[@"kind"];
    NSString *token = [body[@"token"] isKindOfClass:NSString.class] ? body[@"token"] : nil;
    BOOL main = message.frameInfo.isMainFrame;
    if (token && ([kind isEqual:@"hello"] || [kind isEqual:@"answer"])) {
        // A new document in the main frame: its old frames are gone.
        if (main && [kind isEqual:@"hello"]) [self.frames removeAllObjects];
        self.frames[token] = message.frameInfo;
    }
    body[@"main"] = @(main);
    NSString *url = message.frameInfo.request.URL.absoluteString;
    if (url) body[@"frameUrl"] = url;
    if (self.message) self.message(self.ctx, JSON(body).UTF8String);
}

- (void)dealloc {
    if (self.free) self.free(self.ctx);
}

@end

static ABXPage *Bridge(void *webview) {
    return objc_getAssociatedObject((__bridge WKWebView *)webview, &kBridgeKey);
}

/// Add the page script and its handler to `webview`, before its first
/// navigation. `message` gets each message's JSON on the main thread;
/// `free_ctx` runs when the webview goes.
void abx_page_install(void *webview, const char *script, void *ctx, ABXMessageFn message,
                      ABXFreeFn free_ctx) {
    WKWebView *wv = (__bridge WKWebView *)webview;
    ABXPage *page = [ABXPage new];
    page.webView = wv;
    page.ctx = ctx;
    page.message = message;
    page.free = free_ctx;
    page.frames = [NSMutableDictionary new];
    objc_setAssociatedObject(wv, &kBridgeKey, page, OBJC_ASSOCIATION_RETAIN_NONATOMIC);
    WKUserContentController *ucc = wv.configuration.userContentController;
    // A weak-ish trampoline isn't needed: the handler doesn't hold the webview.
    [ucc addScriptMessageHandler:page contentWorld:WKContentWorld.pageWorld name:kHandler];
    [ucc addUserScript:[[WKUserScript alloc] initWithSource:@(script)
                                              injectionTime:WKUserScriptInjectionTimeAtDocumentStart
                                           forMainFrameOnly:NO]];
}

/// Whether the frame with `token` is known (it may still have gone since).
bool abx_frame_known(void *webview, const char *token) {
    ABXPage *page = Bridge(webview);
    return page && token && page.frames[@(token)] != nil;
}

/// Run `script` in the frame with `token` and hand `done` the JSON of its
/// result, or an error.
void abx_eval_in_frame(void *webview, const char *token, const char *script, void *ctx, ABXDoneFn done) {
    WKWebView *wv = (__bridge WKWebView *)webview;
    ABXPage *page = Bridge(webview);
    WKFrameInfo *frame = page && token ? page.frames[@(token)] : nil;
    if (!frame) {
        done(ctx, NULL, "that frame is gone (run `browser frame main`, then pick it again)");
        return;
    }
    [wv evaluateJavaScript:@(script)
                   inFrame:frame
            inContentWorld:WKContentWorld.pageWorld
         completionHandler:^(id result, NSError *error) {
           if (error) {
               NSString *why = error.userInfo[@"WKJavaScriptExceptionMessage"] ?: error.localizedDescription;
               done(ctx, NULL, why.UTF8String);
           } else {
               done(ctx, JSON(result).UTF8String, NULL);
           }
         }];
}

// ---- cookies ----------------------------------------------------------------

static NSDictionary *CookieJSON(NSHTTPCookie *c) {
    NSMutableDictionary *d = [@{
        @"name" : c.name,
        @"value" : c.value,
        @"domain" : c.domain,
        @"path" : c.path ?: @"/",
        @"expires" : c.expiresDate ? @(floor(c.expiresDate.timeIntervalSince1970)) : @(-1),
        @"httpOnly" : @(c.isHTTPOnly),
        @"secure" : @(c.isSecure),
    } mutableCopy];
    NSString *same = c.sameSitePolicy;
    if ([same isEqual:NSHTTPCookieSameSiteStrict]) d[@"sameSite"] = @"Strict";
    else if ([same isEqual:NSHTTPCookieSameSiteLax]) d[@"sameSite"] = @"Lax";
    else d[@"sameSite"] = @"None";
    return d;
}

/// All the webview's cookies, as a JSON array (Playwright's cookie fields).
void abx_cookies_get(void *webview, void *ctx, ABXDoneFn done) {
    WKWebView *wv = (__bridge WKWebView *)webview;
    [wv.configuration.websiteDataStore.httpCookieStore getAllCookies:^(NSArray<NSHTTPCookie *> *cookies) {
      NSMutableArray *out = [NSMutableArray new];
      for (NSHTTPCookie *c in cookies) [out addObject:CookieJSON(c)];
      done(ctx, JSON(out).UTF8String, NULL);
    }];
}

/// Set the cookies in `json` (an array of {name, value, domain, path,
/// expires, httpOnly, secure, sameSite}). `done` gets the count set.
void abx_cookies_set(void *webview, const char *json, void *ctx, ABXDoneFn done) {
    WKWebView *wv = (__bridge WKWebView *)webview;
    NSArray *list = Parse(json);
    if (![list isKindOfClass:NSArray.class]) {
        done(ctx, NULL, "expected an array of cookies");
        return;
    }
    NSMutableArray<NSHTTPCookie *> *cookies = [NSMutableArray new];
    for (NSDictionary *c in list) {
        if (![c isKindOfClass:NSDictionary.class] || ![c[@"name"] isKindOfClass:NSString.class] ||
            ![c[@"domain"] isKindOfClass:NSString.class]) {
            done(ctx, NULL, "each cookie needs a name and a domain");
            return;
        }
        NSMutableDictionary *p = [@{
            NSHTTPCookieName : c[@"name"],
            NSHTTPCookieValue : [c[@"value"] isKindOfClass:NSString.class] ? c[@"value"] : @"",
            NSHTTPCookieDomain : c[@"domain"],
            NSHTTPCookiePath : [c[@"path"] isKindOfClass:NSString.class] ? c[@"path"] : @"/",
        } mutableCopy];
        NSNumber *expires = [c[@"expires"] isKindOfClass:NSNumber.class] ? c[@"expires"] : nil;
        if (expires && expires.doubleValue > 0)
            p[NSHTTPCookieExpires] = [NSDate dateWithTimeIntervalSince1970:expires.doubleValue];
        if ([c[@"secure"] boolValue]) p[NSHTTPCookieSecure] = @"TRUE";
        if ([c[@"httpOnly"] boolValue]) p[@"HttpOnly"] = @"TRUE";
        NSString *same = [c[@"sameSite"] isKindOfClass:NSString.class] ? [c[@"sameSite"] lowercaseString] : nil;
        if ([same isEqual:@"strict"]) p[NSHTTPCookieSameSitePolicy] = NSHTTPCookieSameSiteStrict;
        else if ([same isEqual:@"lax"]) p[NSHTTPCookieSameSitePolicy] = NSHTTPCookieSameSiteLax;
        NSHTTPCookie *cookie = [NSHTTPCookie cookieWithProperties:p];
        if (!cookie) {
            done(ctx, NULL, [NSString stringWithFormat:@"WebKit won't take cookie %@", c[@"name"]].UTF8String);
            return;
        }
        [cookies addObject:cookie];
    }
    WKHTTPCookieStore *store = wv.configuration.websiteDataStore.httpCookieStore;
    dispatch_group_t group = dispatch_group_create();
    for (NSHTTPCookie *c in cookies) {
        dispatch_group_enter(group);
        [store setCookie:c completionHandler:^{ dispatch_group_leave(group); }];
    }
    dispatch_group_notify(group, dispatch_get_main_queue(), ^{
      done(ctx, JSON(@(cookies.count)).UTF8String, NULL);
    });
}

/// Delete the cookies matching `json` (an array of {name, domain, path}).
/// `done` gets the count deleted.
void abx_cookies_delete(void *webview, const char *json, void *ctx, ABXDoneFn done) {
    WKWebView *wv = (__bridge WKWebView *)webview;
    NSArray *want = Parse(json);
    if (![want isKindOfClass:NSArray.class]) {
        done(ctx, NULL, "expected an array of cookies");
        return;
    }
    WKHTTPCookieStore *store = wv.configuration.websiteDataStore.httpCookieStore;
    [store getAllCookies:^(NSArray<NSHTTPCookie *> *cookies) {
      dispatch_group_t group = dispatch_group_create();
      NSUInteger n = 0;
      for (NSHTTPCookie *c in cookies) {
          BOOL hit = NO;
          for (NSDictionary *w in want) {
              if ([w isKindOfClass:NSDictionary.class] && [w[@"name"] isEqual:c.name] &&
                  [w[@"domain"] isEqual:c.domain] && [w[@"path"] isEqual:c.path ?: @"/"]) {
                  hit = YES;
                  break;
              }
          }
          if (!hit) continue;
          n++;
          dispatch_group_enter(group);
          [store deleteCookie:c completionHandler:^{ dispatch_group_leave(group); }];
      }
      dispatch_group_notify(group, dispatch_get_main_queue(), ^{
        done(ctx, JSON(@(n)).UTF8String, NULL);
      });
    }];
}

// ---- the link under the pointer -------------------------------------------

/// Show `url` at the page's bottom-left corner, as Safari does, or hide the
/// label for NULL or "".
void abx_hover(void *webview, const char *url) {
    WKWebView *wv = (__bridge WKWebView *)webview;
    ABXPage *page = Bridge(webview);
    if (!page) return;
    NSString *text = url ? @(url) : @"";
    if (!text.length) {
        page.hoverBox.hidden = YES;
        return;
    }
    if (!page.hoverBox) {
        ABXPassthroughView *box = [ABXPassthroughView new];
        box.wantsLayer = YES;
        box.layer.backgroundColor = [NSColor colorWithWhite:0.13 alpha:0.94].CGColor;
        box.layer.borderColor = [NSColor colorWithWhite:1 alpha:0.12].CGColor;
        box.layer.borderWidth = 1;
        box.layer.cornerRadius = 5;
        NSTextField *label = [NSTextField labelWithString:@""];
        label.font = [NSFont systemFontOfSize:11];
        label.textColor = [NSColor colorWithWhite:0.9 alpha:1];
        label.lineBreakMode = NSLineBreakByTruncatingMiddle;
        label.maximumNumberOfLines = 1;
        [box addSubview:label];
        page.hoverBox = box;
        page.hoverLabel = label;
    }
    if (page.hoverBox.superview != wv) [wv addSubview:page.hoverBox positioned:NSWindowAbove relativeTo:nil];
    page.hoverLabel.stringValue = text;
    NSSize fit = page.hoverLabel.fittingSize;
    NSRect b = wv.bounds;
    const CGFloat padX = 7, padY = 3, margin = 4;
    CGFloat maxW = MAX(160, b.size.width * 0.6) - 2 * padX;
    CGFloat w = MIN(ceil(fit.width), maxW), h = ceil(fit.height);
    CGFloat boxH = h + 2 * padY;
    CGFloat y = wv.isFlipped ? b.size.height - boxH - margin : margin;
    page.hoverBox.frame = NSMakeRect(margin, y, w + 2 * padX, boxH);
    page.hoverLabel.frame = NSMakeRect(padX, padY, w, h);
    page.hoverBox.autoresizingMask = NSViewMaxXMargin | (wv.isFlipped ? NSViewMinYMargin : NSViewMaxYMargin);
    page.hoverBox.hidden = NO;
}

// ---- a bitmap of the page --------------------------------------------------

/// The page as shown, as premultiplied BGRA pixels at the screen's scale.
/// `done` gets NULL if WebKit can't take it.
void abx_snapshot(void *webview, void *ctx, ABXImageFn done) {
    WKWebView *wv = (__bridge WKWebView *)webview;
    [wv takeSnapshotWithConfiguration:nil
                    completionHandler:^(NSImage *image, NSError *__unused error) {
                      CGImageRef cg = image ? [image CGImageForProposedRect:NULL context:nil hints:nil] : NULL;
                      if (!cg) {
                          done(ctx, NULL, 0, 0);
                          return;
                      }
                      size_t w = CGImageGetWidth(cg), h = CGImageGetHeight(cg);
                      uint8_t *buf = calloc(w * h, 4);
                      CGColorSpaceRef space = CGColorSpaceCreateDeviceRGB();
                      CGContextRef bc = CGBitmapContextCreate(
                          buf, w, h, 8, w * 4, space,
                          (CGBitmapInfo)kCGImageAlphaPremultipliedFirst | kCGBitmapByteOrder32Little);
                      CGColorSpaceRelease(space);
                      if (!bc) {
                          free(buf);
                          done(ctx, NULL, 0, 0);
                          return;
                      }
                      CGContextDrawImage(bc, CGRectMake(0, 0, w, h), cg);
                      CGContextRelease(bc);
                      done(ctx, buf, (uint32_t)w, (uint32_t)h);
                      free(buf);
                    }];
}

// ---- downloads -------------------------------------------------------------

/// Downloads by their request's URL (as wry reports them), until
/// `abx_download_forget`.
static NSMapTable<NSString *, WKDownload *> *Downloads(void) {
    static NSMapTable *map;
    static dispatch_once_t once;
    dispatch_once(&once, ^{ map = [NSMapTable strongToStrongObjectsMapTable]; });
    return map;
}

static void Track(WKDownload *download) {
    NSString *url = download.originalRequest.URL.absoluteString;
    if (url) [Downloads() setObject:download forKey:url];
}

static void Wrap3(Class cls, SEL sel) {
    Method m = class_getInstanceMethod(cls, sel);
    if (!m) return;
    void (*orig)(id, SEL, id, id, id) = (void *)method_getImplementation(m);
    id block = ^(id me, id webView, id from, WKDownload *download) {
      orig(me, sel, webView, from, download);
      Track(download);
    };
    method_setImplementation(m, imp_implementationWithBlock(block));
}

/// Hook wry's navigation delegate (once): note each download's WKDownload,
/// and save `Content-Disposition: attachment` responses rather than show
/// them, as browsers do (wry only downloads what WebKit can't show).
void abx_downloads_install(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
      Class cls = NSClassFromString(@"WryNavigationDelegate");
      if (!cls) return;
      Wrap3(cls, @selector(webView:navigationResponse:didBecomeDownload:));
      Wrap3(cls, @selector(webView:navigationAction:didBecomeDownload:));
      SEL sel = @selector(webView:decidePolicyForNavigationResponse:decisionHandler:);
      Method m = class_getInstanceMethod(cls, sel);
      if (!m) return;
      void (*orig)(id, SEL, id, id, id) = (void *)method_getImplementation(m);
      id block = ^(id me, WKWebView *webView, WKNavigationResponse *response,
                   void (^handler)(WKNavigationResponsePolicy)) {
        NSURLResponse *r = response.response;
        if ([r isKindOfClass:NSHTTPURLResponse.class]) {
            NSString *cd = [(NSHTTPURLResponse *)r valueForHTTPHeaderField:@"Content-Disposition"];
            NSString *kind = [[cd componentsSeparatedByString:@";"].firstObject
                stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceCharacterSet].lowercaseString;
            if ([kind isEqual:@"attachment"]) {
                handler(WKNavigationResponsePolicyDownload);
                return;
            }
        }
        orig(me, sel, webView, response, handler);
      };
      method_setImplementation(m, imp_implementationWithBlock(block));
    });
}

/// The bytes written and expected (-1 if unknown) of the download from
/// `url`, while it runs. False once it's over (or unknown).
bool abx_download_progress(const char *url, int64_t *written, int64_t *total) {
    WKDownload *d = url ? [Downloads() objectForKey:@(url)] : nil;
    if (!d) return false;
    NSProgress *p = d.progress;
    *written = p.completedUnitCount;
    *total = p.totalUnitCount;
    return !p.finished && !p.cancelled;
}

/// Stop the download from `url`. False if it isn't running.
bool abx_download_cancel(const char *url) {
    WKDownload *d = url ? [Downloads() objectForKey:@(url)] : nil;
    if (!d) return false;
    [d cancel:^(NSData *__unused resumeData) {}];
    return true;
}

/// Let go of the download from `url`, once it's over.
void abx_download_forget(const char *url) {
    if (url) [Downloads() removeObjectForKey:@(url)];
}

// ---- app shortcuts while a page has focus ----------------------------------

static NSView *GPUIView(NSView *v) {
    if (!v) return nil;
    if ([NSStringFromClass(v.class) isEqual:@"GPUIView"]) return v;
    for (NSView *s in v.subviews) {
        NSView *found = GPUIView(s);
        if (found) return found;
    }
    return nil;
}

static BOOL InWebView(NSResponder *r) {
    for (NSView *v = [r isKindOfClass:NSView.class] ? (NSView *)r : nil; v; v = v.superview) {
        if ([v isKindOfClass:WKWebView.class]) return YES;
    }
    return NO;
}

/// While a page has keyboard focus, WebKit takes every key, ⌘ ones too, and
/// the app never sees them. So (once) watch key presses with ⌘ or ⌃ and
/// offer them to gpui's view first: one the app handles (⌘T, ⌘W, ⌘=, ⌘⇧P…)
/// stops there, anything else (⌘C, a site's own shortcuts) goes on to the page.
void abx_page_shortcuts_install(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
      [NSEvent addLocalMonitorForEventsMatchingMask:NSEventMaskKeyDown
                                            handler:^NSEvent *(NSEvent *e) {
                                              NSEventModifierFlags m =
                                                  e.modifierFlags & NSEventModifierFlagDeviceIndependentFlagsMask;
                                              if (!(m & (NSEventModifierFlagCommand | NSEventModifierFlagControl)))
                                                  return e;
                                              NSWindow *w = e.window;
                                              if (!w || !InWebView(w.firstResponder)) return e;
                                              NSView *g = GPUIView(w.contentView);
                                              if (!g) return e;
                                              return [g performKeyEquivalent:e] ? nil : e;
                                            }];
    });
}

typedef bool (*ABXOpenURLFn)(const char *url);

/// gpui opens links with -[NSWorkspace openURL:], and gpui-component's
/// markdown calls it on a click with no way to say otherwise. So (once)
/// route every call through `hook` first: true from it means abromux took
/// the URL (to show in a browser tab) and the system browser isn't asked.
void abx_open_url_hook_install(ABXOpenURLFn hook) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
      SEL sel = @selector(openURL:);
      Method m = class_getInstanceMethod(NSWorkspace.class, sel);
      if (!m) return;
      BOOL (*orig)(id, SEL, NSURL *) = (void *)method_getImplementation(m);
      id block = ^BOOL(id me, NSURL *url) {
        const char *s = url.absoluteString.UTF8String;
        if (s && hook(s)) return YES;
        return orig(me, sel, url);
      };
      method_setImplementation(m, imp_implementationWithBlock(block));
    });
}
