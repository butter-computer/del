// A small WebKit browser hosted in the GPUI sidebar. All bridge calls are on the UI thread.
#import <AppKit/AppKit.h>
#import <WebKit/WebKit.h>

static NSURL *DelBrowserURL(NSString *address) {
    NSString *text = [address stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
    if (!text.length || [text isEqualToString:@"about:blank"]) return [NSURL URLWithString:@"about:blank"];
    if (![text containsString:@"://"]) {
        BOOL local = [text isEqualToString:@"localhost"] || [text hasPrefix:@"localhost:"] ||
            [text hasPrefix:@"localhost/"] || [text hasPrefix:@"127."] || [text hasPrefix:@"[::1]"];
        // A bare hostname/port is allowed, but arbitrary URL schemes aren't.
        text = [(local ? @"http://" : @"https://") stringByAppendingString:text];
    }
    NSURLComponents *parts = [NSURLComponents componentsWithString:text];
    if (!([parts.scheme.lowercaseString isEqualToString:@"http"] ||
          [parts.scheme.lowercaseString isEqualToString:@"https"]) || !parts.host.length ||
        [parts.host rangeOfCharacterFromSet:NSCharacterSet.whitespaceAndNewlineCharacterSet].location != NSNotFound ||
        parts.user.length || parts.password.length) return nil;
    return parts.URL;
}

@interface DelBrowserView : NSView <WKNavigationDelegate, WKUIDelegate>
@property(nonatomic, strong) WKWebView *web;
@property(nonatomic, strong) NSTextField *address;
@property(nonatomic, strong) NSTextField *status;
@property(nonatomic, strong) NSTextField *welcome;
@property(nonatomic, strong) NSButton *back;
@property(nonatomic, strong) NSButton *forward;
@property(nonatomic, strong) NSButton *reload;
@property(nonatomic, copy) NSString *savedURL;
@property(nonatomic) CGFloat zoom;
@property(nonatomic) BOOL mousePassthrough;
- (void)navigate:(NSString *)address;
- (void)refreshControls;
@end

@implementation DelBrowserView
- (BOOL)isFlipped { return YES; }
- (NSView *)hitTest:(NSPoint)point {
    // GPUI owns ongoing splitter/tab drags, even over the native web view.
    return self.mousePassthrough ? nil : [super hitTest:point];
}
- (NSButton *)button:(NSString *)symbol label:(NSString *)label action:(SEL)action {
    NSButton *button = [NSButton buttonWithImage:[NSImage imageWithSystemSymbolName:symbol accessibilityDescription:label]
                                        target:self action:action];
    button.bordered = NO;
    button.toolTip = label;
    [self addSubview:button];
    return button;
}
- (instancetype)initWithFrame:(NSRect)frame profile:(NSUUID *)profile {
    if (!(self = [super initWithFrame:frame])) return nil;
    _zoom = 1;
    _savedURL = @"about:blank";
    self.wantsLayer = YES;
    self.layer.masksToBounds = YES;
    WKWebViewConfiguration *configuration = [WKWebViewConfiguration new];
    if (@available(macOS 14.0, *)) {
        configuration.websiteDataStore = [WKWebsiteDataStore dataStoreForIdentifier:profile];
    } else { return nil; }
    _web = [[WKWebView alloc] initWithFrame:NSZeroRect configuration:configuration];
    _web.navigationDelegate = self;
    _web.UIDelegate = self;
    _web.allowsBackForwardNavigationGestures = YES;
    _web.hidden = YES;
    [self addSubview:_web];
    _welcome = [NSTextField labelWithString:@"Enter a URL to browse"];
    _welcome.alignment = NSTextAlignmentCenter;
    _welcome.textColor = NSColor.secondaryLabelColor;
    [self addSubview:_welcome];
    _back = [self button:@"chevron.left" label:@"Back" action:@selector(goBack:)];
    _forward = [self button:@"chevron.right" label:@"Forward" action:@selector(goForward:)];
    _reload = [self button:@"arrow.clockwise" label:@"Reload" action:@selector(reloadPage:)];
    _address = [[NSTextField alloc] initWithFrame:NSZeroRect];
    _address.placeholderString = @"URL or localhost:3000";
    _address.bezelStyle = NSTextFieldRoundedBezel;
    _address.target = self;
    _address.action = @selector(openAddress:);
    _address.accessibilityLabel = @"Browser address";
    _address.lineBreakMode = NSLineBreakByTruncatingMiddle;
    [self addSubview:_address];
    _status = [NSTextField labelWithString:@""];
    _status.textColor = NSColor.secondaryLabelColor;
    _status.lineBreakMode = NSLineBreakByTruncatingTail;
    _status.hidden = YES;
    [self addSubview:_status];
    [self refreshControls];
    return self;
}
- (void)layout {
    [super layout];
    CGFloat z = self.zoom, w = NSWidth(self.bounds), h = NSHeight(self.bounds);
    CGFloat bar = 38*z, button = 25*z, inset = 8*z;
    self.back.frame = NSMakeRect(inset, 6*z, button, 26*z);
    self.forward.frame = NSMakeRect(inset+button, 6*z, button, 26*z);
    self.reload.frame = NSMakeRect(inset+button*2, 6*z, button, 26*z);
    self.address.frame = NSMakeRect(inset+button*3+4*z, 6*z, MAX(0,w-inset*2-button*3-4*z), 26*z);
    CGFloat footer = self.status.hidden ? 0 : 24*z;
    self.web.frame = NSMakeRect(0, bar, w, MAX(0,h-bar-footer));
    self.status.frame = NSMakeRect(inset, MAX(bar,h-footer+3*z), MAX(0,w-inset*2), 18*z);
    self.welcome.frame = NSMakeRect(inset, MAX(bar,(h-20*z)/2), MAX(0,w-inset*2), 22*z);
    self.address.font = [NSFont systemFontOfSize:12*z];
    self.status.font = [NSFont systemFontOfSize:11*z];
    self.welcome.font = [NSFont systemFontOfSize:13*z];
}
- (void)showStatus:(NSString *)message {
    self.status.stringValue = message ?: @"";
    self.status.toolTip = message;
    self.status.hidden = !message.length;
    self.needsLayout = YES;
}
- (void)refreshControls {
    self.back.enabled = self.web.canGoBack;
    self.forward.enabled = self.web.canGoForward;
    self.reload.enabled = ![self.savedURL isEqualToString:@"about:blank"];
    self.reload.image = [NSImage imageWithSystemSymbolName:self.web.loading ? @"xmark" : @"arrow.clockwise"
                                 accessibilityDescription:self.web.loading ? @"Stop loading" : @"Reload"];
    self.reload.toolTip = self.web.loading ? @"Stop loading" : @"Reload";
    NSString *url = self.web.URL.absoluteString;
    if (url.length && ![url isEqualToString:@"about:blank"]) self.savedURL = url;
    if (!self.address.currentEditor) {
        self.address.stringValue = [self.savedURL isEqualToString:@"about:blank"] ? @"" : self.savedURL;
    }
}
- (void)navigate:(NSString *)address {
    NSURL *url = DelBrowserURL(address);
    if (!url) { [self showStatus:@"Enter a valid HTTP or HTTPS address."]; return; }
    self.savedURL = url.absoluteString;
    self.address.stringValue = [self.savedURL isEqualToString:@"about:blank"] ? @"" : self.savedURL;
    self.welcome.hidden = ![self.savedURL isEqualToString:@"about:blank"];
    self.web.hidden = !self.welcome.hidden;
    [self showStatus:self.web.hidden ? nil : @"Loading…"];
    [self.web loadRequest:[NSURLRequest requestWithURL:url]];
}
- (void)openAddress:(id)sender { [self navigate:self.address.stringValue]; [self.window makeFirstResponder:self.web]; }
- (void)goBack:(id)sender { [self.web goBack]; }
- (void)goForward:(id)sender { [self.web goForward]; }
- (void)reloadPage:(id)sender {
    if (self.web.loading) { [self.web stopLoading]; [self showStatus:nil]; }
    else if (self.web.URL) [self.web reload];
    else [self navigate:self.savedURL];
    [self refreshControls];
}
- (void)webView:(WKWebView *)web didStartProvisionalNavigation:(WKNavigation *)navigation {
    [self showStatus:@"Loading…"]; [self refreshControls];
}
- (void)webView:(WKWebView *)web didCommitNavigation:(WKNavigation *)navigation { [self refreshControls]; }
- (void)webView:(WKWebView *)web didFinishNavigation:(WKNavigation *)navigation {
    [self showStatus:nil]; [self refreshControls];
}
- (void)failed:(NSError *)error {
    if (error.code != NSURLErrorCancelled) [self showStatus:[@"Couldn't load page: " stringByAppendingString:error.localizedDescription]];
    [self refreshControls];
}
- (void)webView:(WKWebView *)web didFailNavigation:(WKNavigation *)navigation withError:(NSError *)error { [self failed:error]; }
- (void)webView:(WKWebView *)web didFailProvisionalNavigation:(WKNavigation *)navigation withError:(NSError *)error { [self failed:error]; }
- (void)webViewWebContentProcessDidTerminate:(WKWebView *)web { [self showStatus:@"Page stopped. Press Reload to try again."]; }
- (void)webView:(WKWebView *)web decidePolicyForNavigationAction:(WKNavigationAction *)action decisionHandler:(void (^)(WKNavigationActionPolicy))decisionHandler {
    NSString *scheme = action.request.URL.scheme.lowercaseString;
    BOOL allowed = [scheme isEqualToString:@"http"] || [scheme isEqualToString:@"https"] ||
        [action.request.URL.absoluteString isEqualToString:@"about:blank"];
    if (!allowed) [self showStatus:@"This pane supports HTTP and HTTPS pages."];
    decisionHandler(allowed ? WKNavigationActionPolicyAllow : WKNavigationActionPolicyCancel);
}
- (void)webView:(WKWebView *)web decidePolicyForNavigationResponse:(WKNavigationResponse *)response decisionHandler:(void (^)(WKNavigationResponsePolicy))decisionHandler {
    if (!response.canShowMIMEType) [self showStatus:@"This file can't be displayed in the browser."];
    decisionHandler(response.canShowMIMEType ? WKNavigationResponsePolicyAllow : WKNavigationResponsePolicyCancel);
}
- (WKWebView *)webView:(WKWebView *)web createWebViewWithConfiguration:(WKWebViewConfiguration *)configuration forNavigationAction:(WKNavigationAction *)action windowFeatures:(WKWindowFeatures *)features {
    // This is a single-page sidebar: target=_blank stays in the same pane.
    if (!action.targetFrame) [web loadRequest:action.request];
    return nil;
}
- (void)webView:(WKWebView *)web runJavaScriptAlertPanelWithMessage:(NSString *)message initiatedByFrame:(WKFrameInfo *)frame completionHandler:(void (^)(void))completionHandler {
    NSAlert *alert = [NSAlert new]; alert.messageText = message; [alert addButtonWithTitle:@"OK"];
    [alert beginSheetModalForWindow:self.window completionHandler:^(NSModalResponse response) { (void)response; completionHandler(); }];
}
- (void)webView:(WKWebView *)web runJavaScriptConfirmPanelWithMessage:(NSString *)message initiatedByFrame:(WKFrameInfo *)frame completionHandler:(void (^)(BOOL))completionHandler {
    NSAlert *alert = [NSAlert new]; alert.messageText = message;
    [alert addButtonWithTitle:@"OK"]; [alert addButtonWithTitle:@"Cancel"];
    [alert beginSheetModalForWindow:self.window completionHandler:^(NSModalResponse response) { completionHandler(response == NSAlertFirstButtonReturn); }];
}
- (void)webView:(WKWebView *)web runJavaScriptTextInputPanelWithPrompt:(NSString *)prompt defaultText:(NSString *)text initiatedByFrame:(WKFrameInfo *)frame completionHandler:(void (^)(NSString *))completionHandler {
    NSAlert *alert = [NSAlert new]; alert.messageText = prompt;
    [alert addButtonWithTitle:@"OK"]; [alert addButtonWithTitle:@"Cancel"];
    NSTextField *input = [[NSTextField alloc] initWithFrame:NSMakeRect(0,0,260,24)]; input.stringValue = text ?: @"";
    alert.accessoryView = input;
    [alert beginSheetModalForWindow:self.window completionHandler:^(NSModalResponse response) { completionHandler(response == NSAlertFirstButtonReturn ? input.stringValue : nil); }];
}
@end

void *del_browser_create(void *parentPointer, const char *initialURL, const char *profileID) {
    if (![NSThread isMainThread] || !parentPointer) return NULL;
    NSUUID *profile = profileID ? [[NSUUID alloc] initWithUUIDString:[NSString stringWithUTF8String:profileID]] : nil;
    if (!profile) return NULL;
    DelBrowserView *browser = [[DelBrowserView alloc] initWithFrame:NSZeroRect profile:profile];
    if (!browser) return NULL;
    browser.hidden = YES;
    [(__bridge NSView *)parentPointer addSubview:browser];
    NSString *url = initialURL ? [NSString stringWithUTF8String:initialURL] : nil;
    if (url.length && ![url isEqualToString:@"about:blank"]) [browser navigate:url];
    return (__bridge_retained void *)browser;
}
void del_browser_frame(void *pointer, double x, double y, double width, double height, double zoom, bool light, bool mousePassthrough) {
    if (![NSThread isMainThread] || !pointer) return;
    DelBrowserView *browser = (__bridge DelBrowserView *)pointer;
    NSView *parent = browser.superview;
    if (!parent || width <= 0 || height <= 0) { browser.hidden = YES; return; }
    if (!parent.isFlipped) y = NSHeight(parent.bounds) - y - height;
    browser.frame = NSMakeRect(x,y,width,height);
    browser.appearance = [NSAppearance appearanceNamed:light ? NSAppearanceNameAqua : NSAppearanceNameDarkAqua];
    browser.layer.backgroundColor = (light ? [NSColor colorWithWhite:0.98 alpha:1] : [NSColor colorWithWhite:0.085 alpha:1]).CGColor;
    browser.zoom = zoom;
    browser.mousePassthrough = mousePassthrough;
    if (browser.web.pageZoom != zoom) browser.web.pageZoom = zoom;
    browser.needsLayout = YES;
    browser.hidden = NO;
}
const char *del_browser_url(void *pointer) {
    if (![NSThread isMainThread] || !pointer) return NULL;
    return ((__bridge DelBrowserView *)pointer).savedURL.UTF8String;
}
void del_browser_hide(void *pointer) {
    if (![NSThread isMainThread] || !pointer) return;
    DelBrowserView *browser = (__bridge DelBrowserView *)pointer;
    NSResponder *first = browser.window.firstResponder;
    if ([first isKindOfClass:NSView.class] && [(NSView *)first isDescendantOf:browser])
        [browser.window makeFirstResponder:browser.superview];
    browser.hidden = YES;
}
void del_browser_release(void *pointer) {
    if (![NSThread isMainThread] || !pointer) return;
    del_browser_hide(pointer);
    DelBrowserView *browser = (__bridge_transfer DelBrowserView *)pointer;
    [browser.web stopLoading];
    browser.web.navigationDelegate = nil;
    browser.web.UIDelegate = nil;
    [browser.web removeFromSuperview];
    browser.web = nil;
    [browser removeFromSuperview];
}

// Tool results return JSON through a one-shot callback; no native app capabilities
// are exposed to page JavaScript.
typedef void (*DelBrowserReply)(void *, const char *);
static void DelReply(DelBrowserReply reply, void *context, id value, NSString *error) {
    NSDictionary *result = error ? @{@"error":error} : @{@"result":value ?: NSNull.null};
    NSData *data = [NSJSONSerialization dataWithJSONObject:result options:NSJSONWritingFragmentsAllowed error:nil];
    if (data.length > 6*1024*1024) { reply(context, "{\"error\":\"Result too large; return a smaller selection\"}"); return; }
    NSString *json = [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding];
    reply(context, (json ?: @"{\"error\":\"Result could not be serialized\"}").UTF8String);
}
void del_browser_command(void *pointer, const char *command, DelBrowserReply reply, void *context) {
    if (![NSThread isMainThread] || !pointer) { DelReply(reply, context, nil, @"Browser unavailable (macOS 14 or newer required)"); return; }
    DelBrowserView *browser = (__bridge DelBrowserView *)pointer;
    NSDictionary *request = [NSJSONSerialization JSONObjectWithData:[[NSString stringWithUTF8String:command] dataUsingEncoding:NSUTF8StringEncoding] options:0 error:nil];
    NSString *action = request[@"action"];
    if ([action isEqualToString:@"navigate"]) {
        if (!DelBrowserURL(request[@"url"])) { DelReply(reply,context,nil,@"Invalid URL"); return; }
        [browser navigate:request[@"url"]];
        DelReply(reply,context,@{@"url":browser.savedURL,@"loading":@YES},nil); return;
    }
    if ([action isEqualToString:@"back"]) { [browser goBack:nil]; DelReply(reply,context,@YES,nil); return; }
    if ([action isEqualToString:@"forward"]) { [browser goForward:nil]; DelReply(reply,context,@YES,nil); return; }
    if ([action isEqualToString:@"reload"]) { [browser.web reload]; DelReply(reply,context,@YES,nil); return; }
    if ([action isEqualToString:@"screenshot"]) {
        WKSnapshotConfiguration *configuration = [WKSnapshotConfiguration new];
        configuration.snapshotWidth = @(MIN(1280, NSWidth(browser.web.bounds)));
        [browser.web takeSnapshotWithConfiguration:configuration completionHandler:^(NSImage *image, NSError *error) {
            if (error || !image) { DelReply(reply,context,nil,error.localizedDescription ?: @"No screenshot available"); return; }
            NSBitmapImageRep *bitmap = [[NSBitmapImageRep alloc] initWithData:image.TIFFRepresentation];
            NSData *png = [bitmap representationUsingType:NSBitmapImageFileTypePNG properties:@{}];
            if (png.length > 4*1024*1024) { DelReply(reply,context,nil,@"Screenshot too large; zoom out first"); return; }
            DelReply(reply,context,@{@"image": [png base64EncodedStringWithOptions:0], @"mimeType":@"image/png"},nil);
        }]; return;
    }
    NSString *script = request[@"script"];
    if (!script) { DelReply(reply,context,nil,@"Missing script"); return; }
    [browser.web callAsyncJavaScript:script arguments:@{} inFrame:nil inContentWorld:WKContentWorld.pageWorld completionHandler:^(id value, NSError *error) {
        if (error) { DelReply(reply,context,nil,error.localizedDescription); return; }
        if ([value isKindOfClass:NSString.class] && [(NSString *)value length] > 200000) {
            DelReply(reply,context,nil,@"Result too large; return a smaller selection"); return;
        }
        DelReply(reply,context,value,nil);
    }];
}
void del_browser_delete_profile(const char *profileID, DelBrowserReply reply, void *context) {
    NSUUID *profile = [[NSUUID alloc] initWithUUIDString:[NSString stringWithUTF8String:profileID]];
    if (!profile) { DelReply(reply,context,nil,@"Invalid browser profile"); return; }
    if (@available(macOS 14.0, *)) {
        [WKWebsiteDataStore removeDataStoreForIdentifier:profile completionHandler:^(NSError *error) {
            if (!error) { DelReply(reply,context,@YES,nil); return; }
            // Erase website data even if WebKit is still retiring a closed view.
            // Keep the deletion journal so the empty store is removed next launch.
            WKWebsiteDataStore *store = [WKWebsiteDataStore dataStoreForIdentifier:profile];
            [store removeDataOfTypes:WKWebsiteDataStore.allWebsiteDataTypes modifiedSince:NSDate.distantPast completionHandler:^{
                DelReply(reply,context,nil,error.localizedDescription);
            }];
        }];
    } else { DelReply(reply,context,nil,@"macOS 14 or newer required"); }
}
