// Native side of the WebAuthn bridge for browser panes (page side: webauthn.js).
//
// WKWebView only does WebAuthn for an app's own associated domains. A browser
// gets passkeys and security keys for any site through AuthenticationServices'
// web-browser APIs, which take the page's origin and challenge as client data.
// They need the restricted com.apple.developer.web-browser.public-key-credential
// entitlement (granted per App ID by Apple, carried by a provisioning profile).
// Without it every request falls back to WebKit, i.e. the old behaviour.
//
// Modelled on cmux's BrowserWebAuthnSupport.swift.

#import <AppKit/AppKit.h>
#import <AuthenticationServices/AuthenticationServices.h>
#import <CoreBluetooth/CoreBluetooth.h>
#import <Security/Security.h>
#import <WebKit/WebKit.h>

static NSString *const kHandler = @"abromuxWebAuthn";

static BOOL HasBrowserEntitlement(void) {
    static BOOL has;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        SecTaskRef task = SecTaskCreateFromSelf(NULL);
        if (!task) return;
        CFTypeRef value = SecTaskCopyValueForEntitlement(
            task, CFSTR("com.apple.developer.web-browser.public-key-credential"), NULL);
        has = value && CFGetTypeID(value) == CFBooleanGetTypeID() && CFBooleanGetValue(value);
        if (value) CFRelease(value);
        CFRelease(task);
    });
    return has;
}

static BOOL NativeEnabled(void) {
    if (@available(macOS 13.5, *)) return HasBrowserEntitlement();
    return NO;
}

static NSDictionary *Fail(NSString *name, NSString *message) {
    return @{@"ok" : @NO, @"error" : @{@"name" : name, @"message" : message}};
}

static NSDictionary *Fallback(void) { return @{@"ok" : @YES, @"fallback" : @YES}; }

static NSString *Str(id v) { return [v isKindOfClass:NSString.class] ? v : nil; }
static NSArray *Arr(id v) { return [v isKindOfClass:NSArray.class] ? v : @[]; }

static NSData *B64Decode(id v) {
    NSString *s = Str(v);
    if (!s) return nil;
    NSMutableString *m = [[s stringByReplacingOccurrencesOfString:@"-" withString:@"+"]
        stringByReplacingOccurrencesOfString:@"_" withString:@"/"].mutableCopy;
    while (m.length % 4) [m appendString:@"="];
    return [[NSData alloc] initWithBase64EncodedString:m options:0];
}

static NSString *B64Encode(NSData *d) {
    if (!d) return nil;
    return [[[[d base64EncodedStringWithOptions:0]
        stringByReplacingOccurrencesOfString:@"+" withString:@"-"]
        stringByReplacingOccurrencesOfString:@"/" withString:@"_"]
        stringByReplacingOccurrencesOfString:@"=" withString:@""];
}

/// The serialized origin ("https://host[:port]") of a frame WebAuthn may run
/// in: https, or http on a loopback host. Nil otherwise.
static NSString *SecureOrigin(WKSecurityOrigin *o) {
    NSString *scheme = o.protocol.lowercaseString, *host = o.host.lowercaseString;
    if (host.length == 0) return nil;
    if ([host hasPrefix:@"["] && [host hasSuffix:@"]"])
        host = [host substringWithRange:NSMakeRange(1, host.length - 2)];
    BOOL loopback = [host isEqual:@"localhost"] || [host hasSuffix:@".localhost"] ||
                    [host isEqual:@"::1"] || [host hasPrefix:@"127."];
    if (!([scheme isEqual:@"https"] || ([scheme isEqual:@"http"] && loopback))) return nil;
    NSString *h = [host containsString:@":"] ? [NSString stringWithFormat:@"[%@]", host] : host;
    BOOL defaultPort = o.port == 0 || ([scheme isEqual:@"https"] && o.port == 443) ||
                       ([scheme isEqual:@"http"] && o.port == 80);
    return defaultPort ? [NSString stringWithFormat:@"%@://%@", scheme, h]
                       : [NSString stringWithFormat:@"%@://%@:%ld", scheme, h, (long)o.port];
}

/// The RP ID to use for a request from `host`: the requested one when it's
/// the host or a parent domain of it (not a bare TLD), else nil.
static NSString *RelyingParty(NSString *requested, NSString *host) {
    NSString *rp = requested.length ? requested.lowercaseString : host;
    if ([rp isEqual:host]) return rp;
    if ([rp containsString:@"."] && [host hasSuffix:[@"." stringByAppendingString:rp]]) return rp;
    return nil;
}

@interface ABXWebAuthn
    : NSObject <WKScriptMessageHandlerWithReply, ASAuthorizationControllerDelegate,
                ASAuthorizationControllerPresentationContextProviding, CBCentralManagerDelegate>
@property(weak) WKWebView *webView;
@property(strong) ASAuthorizationController *controller;
@property(copy) void (^pending)(id, NSString *);
@property(strong) CBCentralManager *bluetooth;
@property(copy) void (^bluetoothReady)(void);
@end

@implementation ABXWebAuthn

- (void)userContentController:(WKUserContentController *)ucc
      didReceiveScriptMessage:(WKScriptMessage *)message
                 replyHandler:(void (^)(id, NSString *))reply {
    NSDictionary *body = [message.body isKindOfClass:NSDictionary.class] ? message.body : @{};
    NSString *kind = Str(body[@"kind"]);
    if ([kind isEqual:@"capabilities"]) {
        reply(@{@"ok" : @YES, @"native" : @(NativeEnabled())}, nil);
        return;
    }
    if ([kind isEqual:@"cancel"]) {
        [self cancel];
        reply(@{@"ok" : @YES}, nil);
        return;
    }
    if (![kind isEqual:@"create"] && ![kind isEqual:@"get"]) {
        reply(Fail(@"TypeError", @"Malformed passkey request."), nil);
        return;
    }
    if (!NativeEnabled() || !message.frameInfo.isMainFrame) {
        reply(Fallback(), nil);
        return;
    }
    if (@available(macOS 13.5, *)) {
        NSString *origin = SecureOrigin(message.frameInfo.securityOrigin);
        if (!origin) {
            reply(Fail(@"SecurityError", @"Passkeys need a secure origin."), nil);
            return;
        }
        NSData *json = [Str(body[@"payload"]) dataUsingEncoding:NSUTF8StringEncoding];
        NSDictionary *opts = json ? [NSJSONSerialization JSONObjectWithData:json options:0 error:nil] : nil;
        if (![opts isKindOfClass:NSDictionary.class]) {
            reply(Fail(@"TypeError", @"Malformed passkey request."), nil);
            return;
        }
        if (self.pending) {
            reply(Fail(@"NotAllowedError", @"Another passkey request is in progress."), nil);
            return;
        }
        NSWindow *window = message.webView.window;
        if (!window || message.webView != self.webView) {
            reply(Fail(@"NotAllowedError", @"The passkey request failed."), nil);
            return;
        }
        NSString *host = message.frameInfo.securityOrigin.host.lowercaseString;
        NSDictionary *failure = nil;
        NSArray *requests = [kind isEqual:@"create"]
                                ? [self createRequests:opts origin:origin host:host failure:&failure]
                                : [self getRequests:opts origin:origin host:host failure:&failure];
        if (failure || requests.count == 0) {
            reply(failure ?: Fallback(), nil);
            return;
        }
        self.pending = reply;
        [self authorize:requests];
    }
}

#pragma mark Requests

- (NSArray *)createRequests:(NSDictionary *)o
                     origin:(NSString *)origin
                       host:(NSString *)host
                    failure:(NSDictionary **)failure API_AVAILABLE(macos(13.5)) {
    NSDictionary *rpInfo = [o[@"rp"] isKindOfClass:NSDictionary.class] ? o[@"rp"] : @{};
    NSDictionary *user = [o[@"user"] isKindOfClass:NSDictionary.class] ? o[@"user"] : @{};
    NSString *rp = RelyingParty(Str(rpInfo[@"id"]), host);
    NSData *challenge = B64Decode(o[@"challenge"]), *userID = B64Decode(user[@"id"]);
    NSString *name = Str(user[@"name"]) ?: @"";
    NSString *displayName = Str(user[@"displayName"]).length ? user[@"displayName"] : name;
    NSArray *algs = Arr(o[@"pubKeyCredParams"]);
    if (!rp) {
        *failure = Fail(@"SecurityError", @"The relying party ID doesn't match this site.");
        return nil;
    }
    if (!challenge || !userID || userID.length == 0 || userID.length > 64 || algs.count == 0) {
        *failure = Fail(@"TypeError", @"Malformed passkey request.");
        return nil;
    }
    ASPublicKeyCredentialClientData *clientData =
        [[ASPublicKeyCredentialClientData alloc] initWithChallenge:challenge origin:origin];
    NSString *attachment = Str(o[@"authenticatorAttachment"]);
    NSString *uv = Str(o[@"userVerification"]) ?: @"preferred";
    NSString *attestation = Str(o[@"attestation"]) ?: @"none";
    NSArray *exclude = Arr(o[@"excludeCredentials"]);
    NSMutableArray *requests = [NSMutableArray array];

    // iCloud Keychain passkeys are ES256 only.
    if (![attachment isEqual:@"cross-platform"] && [algs containsObject:@(-7)]) {
        ASAuthorizationPlatformPublicKeyCredentialProvider *p =
            [[ASAuthorizationPlatformPublicKeyCredentialProvider alloc] initWithRelyingPartyIdentifier:rp];
        ASAuthorizationPlatformPublicKeyCredentialRegistrationRequest *r =
            [p createCredentialRegistrationRequestWithClientData:clientData name:name userID:userID];
        r.displayName = displayName;
        r.userVerificationPreference = uv;
        r.attestationPreference = attestation;
        r.excludedCredentials = [self platformDescriptors:exclude];
        // Hybrid is a phone over Bluetooth: offered unless the site wants this Mac.
        r.shouldShowHybridTransport = ![attachment isEqual:@"platform"];
        [requests addObject:r];
    }
    if (@available(macOS 14.4, *)) {
        if (![attachment isEqual:@"platform"]) {
            ASAuthorizationSecurityKeyPublicKeyCredentialProvider *p =
                [[ASAuthorizationSecurityKeyPublicKeyCredentialProvider alloc] initWithRelyingPartyIdentifier:rp];
            ASAuthorizationSecurityKeyPublicKeyCredentialRegistrationRequest *r =
                [p createCredentialRegistrationRequestWithClientData:clientData
                                                         displayName:displayName
                                                                name:name
                                                              userID:userID];
            NSMutableArray *params = [NSMutableArray array];
            for (id alg in algs) {
                if ([alg isKindOfClass:NSNumber.class])
                    [params addObject:[[ASAuthorizationPublicKeyCredentialParameters alloc]
                                          initWithAlgorithm:[alg integerValue]]];
            }
            r.credentialParameters = params;
            r.userVerificationPreference = uv;
            r.attestationPreference = attestation;
            r.residentKeyPreference = Str(o[@"residentKey"]) ?: @"discouraged";
            r.excludedCredentials = [self securityKeyDescriptors:exclude];
            // Security keys first when the site asked for a roaming authenticator.
            [attachment isEqual:@"cross-platform"] ? [requests insertObject:r atIndex:0]
                                                   : [requests addObject:r];
        }
    }
    return requests;
}

- (NSArray *)getRequests:(NSDictionary *)o
                  origin:(NSString *)origin
                    host:(NSString *)host
                 failure:(NSDictionary **)failure API_AVAILABLE(macos(13.5)) {
    // U2F AppID requests have their own trust rules; leave them to WebKit.
    if (Str(o[@"appid"]).length) return nil;
    NSString *rp = RelyingParty(Str(o[@"rpId"]), host);
    NSData *challenge = B64Decode(o[@"challenge"]);
    if (!rp) {
        *failure = Fail(@"SecurityError", @"The relying party ID doesn't match this site.");
        return nil;
    }
    if (!challenge) {
        *failure = Fail(@"TypeError", @"Malformed passkey request.");
        return nil;
    }
    ASPublicKeyCredentialClientData *clientData =
        [[ASPublicKeyCredentialClientData alloc] initWithChallenge:challenge origin:origin];
    NSString *uv = Str(o[@"userVerification"]) ?: @"preferred";
    NSArray *allow = Arr(o[@"allowCredentials"]);

    // An allow list with transports says where the credentials live.
    BOOL platform = allow.count == 0, securityKey = allow.count == 0, hybrid = allow.count == 0;
    for (NSDictionary *d in allow) {
        NSArray *t = [d isKindOfClass:NSDictionary.class] ? Arr(d[@"transports"]) : @[];
        if (t.count == 0) platform = securityKey = hybrid = YES;
        if ([t containsObject:@"internal"] || [t containsObject:@"hybrid"]) platform = YES;
        if ([t containsObject:@"hybrid"]) hybrid = YES;
        if ([t containsObject:@"usb"] || [t containsObject:@"nfc"] || [t containsObject:@"ble"])
            securityKey = YES;
    }

    NSMutableArray *requests = [NSMutableArray array];
    if (platform) {
        ASAuthorizationPlatformPublicKeyCredentialProvider *p =
            [[ASAuthorizationPlatformPublicKeyCredentialProvider alloc] initWithRelyingPartyIdentifier:rp];
        ASAuthorizationPlatformPublicKeyCredentialAssertionRequest *r =
            [p createCredentialAssertionRequestWithClientData:clientData];
        r.userVerificationPreference = uv;
        NSArray *ids = [self platformDescriptors:allow];
        if (ids.count) r.allowedCredentials = ids;
        r.shouldShowHybridTransport = hybrid;
        [requests addObject:r];
    }
    if (@available(macOS 14.4, *)) {
        if (securityKey) {
            ASAuthorizationSecurityKeyPublicKeyCredentialProvider *p =
                [[ASAuthorizationSecurityKeyPublicKeyCredentialProvider alloc] initWithRelyingPartyIdentifier:rp];
            ASAuthorizationSecurityKeyPublicKeyCredentialAssertionRequest *r =
                [p createCredentialAssertionRequestWithClientData:clientData];
            r.userVerificationPreference = uv;
            NSArray *ids = [self securityKeyDescriptors:allow];
            if (ids.count) r.allowedCredentials = ids;
            platform ? [requests addObject:r] : [requests insertObject:r atIndex:0];
        }
    }
    return requests;
}

- (NSArray *)platformDescriptors:(NSArray *)list {
    NSMutableArray *out = [NSMutableArray array];
    for (NSDictionary *d in list) {
        NSData *cid = [d isKindOfClass:NSDictionary.class] ? B64Decode(d[@"id"]) : nil;
        if (cid)
            [out addObject:[[ASAuthorizationPlatformPublicKeyCredentialDescriptor alloc]
                               initWithCredentialID:cid]];
    }
    return out;
}

- (NSArray *)securityKeyDescriptors:(NSArray *)list {
    NSMutableArray *out = [NSMutableArray array];
    for (NSDictionary *d in list) {
        NSData *cid = [d isKindOfClass:NSDictionary.class] ? B64Decode(d[@"id"]) : nil;
        if (!cid) continue;
        NSMutableArray *transports = [NSMutableArray array];
        for (NSString *t in Arr(d[@"transports"])) {
            if ([t isEqual:@"usb"]) [transports addObject:ASAuthorizationSecurityKeyPublicKeyCredentialDescriptorTransportUSB];
            if ([t isEqual:@"nfc"]) [transports addObject:ASAuthorizationSecurityKeyPublicKeyCredentialDescriptorTransportNFC];
            if ([t isEqual:@"ble"]) [transports addObject:ASAuthorizationSecurityKeyPublicKeyCredentialDescriptorTransportBluetooth];
        }
        if (transports.count == 0) [transports setArray:ASAuthorizationAllSupportedPublicKeyCredentialDescriptorTransports()];
        [out addObject:[[ASAuthorizationSecurityKeyPublicKeyCredentialDescriptor alloc]
                           initWithCredentialID:cid
                                     transports:transports]];
    }
    return out;
}

#pragma mark Ceremony

/// Ask once for passkey access (macOS's "allow this browser" prompt) and for
/// Bluetooth (phones as authenticators), then show the system sheet.
- (void)authorize:(NSArray *)requests API_AVAILABLE(macos(13.5)) {
    ASAuthorizationWebBrowserPublicKeyCredentialManager *manager =
        [[ASAuthorizationWebBrowserPublicKeyCredentialManager alloc] init];
    void (^withAccess)(ASAuthorizationWebBrowserPublicKeyCredentialManagerAuthorizationState) =
        ^(ASAuthorizationWebBrowserPublicKeyCredentialManagerAuthorizationState state) {
            dispatch_async(dispatch_get_main_queue(), ^{
                NSArray *allowed = requests;
                if (state != ASAuthorizationWebBrowserPublicKeyCredentialManagerAuthorizationStateAuthorized) {
                    // Passkeys are off for this app; security keys still work.
                    allowed = [requests filteredArrayUsingPredicate:
                                            [NSPredicate predicateWithBlock:^BOOL(id r, NSDictionary *_) {
                                                return ![r isKindOfClass:ASAuthorizationPlatformPublicKeyCredentialRegistrationRequest.class] &&
                                                       ![r isKindOfClass:ASAuthorizationPlatformPublicKeyCredentialAssertionRequest.class];
                                            }]];
                }
                if (allowed.count == 0) {
                    [self finish:Fail(@"NotAllowedError", @"Passkeys are turned off for this app.")];
                    return;
                }
                BOOL hybrid = NO;
                for (id r in allowed) {
                    if ([r respondsToSelector:@selector(shouldShowHybridTransport)])
                        hybrid |= [r shouldShowHybridTransport];
                }
                [self whenBluetoothReady:hybrid then:^{
                    if (!self.pending) return;  // cancelled meanwhile
                    self.controller = [[ASAuthorizationController alloc] initWithAuthorizationRequests:allowed];
                    self.controller.delegate = self;
                    self.controller.presentationContextProvider = self;
                    [self.controller performRequests];
                }];
            });
        };
    ASAuthorizationWebBrowserPublicKeyCredentialManagerAuthorizationState state =
        manager.authorizationStateForPlatformCredentials;
    if (state == ASAuthorizationWebBrowserPublicKeyCredentialManagerAuthorizationStateNotDetermined)
        [manager requestAuthorizationForPublicKeyCredentials:withAccess];
    else
        withAccess(state);
}

- (void)whenBluetoothReady:(BOOL)needed then:(void (^)(void))ready {
    if (!needed || CBCentralManager.authorization != CBManagerAuthorizationNotDetermined) {
        ready();
        return;
    }
    // Creating a manager is what shows macOS's Bluetooth prompt.
    self.bluetoothReady = ready;
    if (!self.bluetooth)
        self.bluetooth = [[CBCentralManager alloc] initWithDelegate:self queue:nil];
}

- (void)centralManagerDidUpdateState:(CBCentralManager *)central {
    if (CBCentralManager.authorization == CBManagerAuthorizationNotDetermined) return;
    void (^ready)(void) = self.bluetoothReady;
    self.bluetoothReady = nil;
    if (ready) ready();
}

- (void)cancel {
    ASAuthorizationController *c = self.controller;
    self.controller = nil;
    self.bluetoothReady = nil;
    c.delegate = nil;
    if (@available(macOS 13.0, *)) [c cancel];
    [self finish:Fail(@"AbortError", @"The passkey request was cancelled.")];
}

- (void)finish:(NSDictionary *)reply {
    void (^pending)(id, NSString *) = self.pending;
    self.pending = nil;
    self.controller = nil;
    if (pending) pending(reply, nil);
}

- (ASPresentationAnchor)presentationAnchorForAuthorizationController:(ASAuthorizationController *)c {
    return self.webView.window ?: NSApp.keyWindow ?: NSApp.mainWindow;
}

- (void)authorizationController:(ASAuthorizationController *)c
    didCompleteWithAuthorization:(ASAuthorization *)authorization {
    id cred = authorization.credential;
    NSDictionary *result = nil;
    if (@available(macOS 13.5, *)) {
        if ([cred isKindOfClass:ASAuthorizationPlatformPublicKeyCredentialRegistration.class]) {
            ASAuthorizationPlatformPublicKeyCredentialRegistration *r = cred;
            BOOL here = r.attachment == ASAuthorizationPublicKeyCredentialAttachmentPlatform;
            result = [self attestation:r.credentialID
                            clientData:r.rawClientDataJSON
                     attestationObject:r.rawAttestationObject
                            attachment:here ? @"platform" : @"cross-platform"
                            transports:here ? @[ @"hybrid", @"internal" ] : @[ @"hybrid" ]];
        } else if ([cred isKindOfClass:ASAuthorizationPlatformPublicKeyCredentialAssertion.class]) {
            ASAuthorizationPlatformPublicKeyCredentialAssertion *a = cred;
            result = [self assertion:a
                          attachment:a.attachment == ASAuthorizationPublicKeyCredentialAttachmentPlatform
                                         ? @"platform" : @"cross-platform"
                          extensions:@{}];
        }
    }
    if ([cred isKindOfClass:ASAuthorizationSecurityKeyPublicKeyCredentialRegistration.class]) {
        ASAuthorizationSecurityKeyPublicKeyCredentialRegistration *r = cred;
        NSArray *transports = @[];
        if (@available(macOS 14.5, *)) transports = r.transports;
        result = [self attestation:r.credentialID
                        clientData:r.rawClientDataJSON
                 attestationObject:r.rawAttestationObject
                        attachment:@"cross-platform"
                        transports:transports];
    } else if ([cred isKindOfClass:ASAuthorizationSecurityKeyPublicKeyCredentialAssertion.class]) {
        result = [self assertion:cred attachment:@"cross-platform" extensions:@{}];
    }
    [self finish:result ?: Fail(@"UnknownError", @"The passkey request failed.")];
}

- (void)authorizationController:(ASAuthorizationController *)c didCompleteWithError:(NSError *)error {
    BOOL excluded = [error.domain isEqual:ASAuthorizationErrorDomain] && error.code == 1006;
    [self finish:excluded ? Fail(@"InvalidStateError", @"A passkey for this account already exists.")
                          : Fail(@"NotAllowedError", @"The passkey request was cancelled or failed.")];
}

- (NSDictionary *)attestation:(NSData *)credentialID
                   clientData:(NSData *)clientData
            attestationObject:(NSData *)attestationObject
                   attachment:(NSString *)attachment
                   transports:(NSArray *)transports {
    if (!credentialID || !clientData || !attestationObject) return nil;
    return @{
        @"ok" : @YES,
        @"credential" : @{
            @"id" : B64Encode(credentialID),
            @"authenticatorAttachment" : attachment,
            @"responseKind" : @"attestation",
            @"response" : @{
                @"clientDataJSON" : B64Encode(clientData),
                @"attestationObject" : B64Encode(attestationObject),
                @"transports" : transports ?: @[],
            },
            @"clientExtensionResults" : @{},
        },
    };
}

- (NSDictionary *)assertion:(id<ASAuthorizationPublicKeyCredentialAssertion>)a
                 attachment:(NSString *)attachment
                 extensions:(NSDictionary *)extensions {
    if (!a.credentialID || !a.rawClientDataJSON || !a.rawAuthenticatorData || !a.signature) return nil;
    NSMutableDictionary *response = [@{
        @"clientDataJSON" : B64Encode(a.rawClientDataJSON),
        @"authenticatorData" : B64Encode(a.rawAuthenticatorData),
        @"signature" : B64Encode(a.signature),
    } mutableCopy];
    if (a.userID.length) response[@"userHandle"] = B64Encode(a.userID);
    return @{
        @"ok" : @YES,
        @"credential" : @{
            @"id" : B64Encode(a.credentialID),
            @"authenticatorAttachment" : attachment,
            @"responseKind" : @"assertion",
            @"response" : response,
            @"clientExtensionResults" : extensions,
        },
    };
}

@end

/// Add the bridge to `webview` (a WKWebView): the page script and its native
/// handler. Call before the first navigation so the first page gets it too.
void abromux_webauthn_install(void *webview, const char *script) {
    WKWebView *wv = (__bridge WKWebView *)webview;
    WKUserContentController *ucc = wv.configuration.userContentController;
    ABXWebAuthn *bridge = [ABXWebAuthn new];
    bridge.webView = wv;
    // The controller keeps the handler (and so the bridge) alive with the webview.
    [ucc addScriptMessageHandlerWithReply:bridge contentWorld:WKContentWorld.pageWorld name:kHandler];
    [ucc addUserScript:[[WKUserScript alloc] initWithSource:@(script)
                                              injectionTime:WKUserScriptInjectionTimeAtDocumentStart
                                           forMainFrameOnly:YES]];
}
