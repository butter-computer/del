// Compile with AppKit/WebKit. A local HTTP fixture URL is the only argument.
#import "../crates/app/src/browser.m"
static void Pump(void) {
    NSEvent *event;
    while ((event=[NSApp nextEventMatchingMask:NSEventMaskAny untilDate:NSDate.distantPast inMode:NSDefaultRunLoopMode dequeue:YES])) [NSApp sendEvent:event];
    [NSApp updateWindows];
    [[NSRunLoop currentRunLoop] runMode:NSDefaultRunLoopMode beforeDate:[NSDate dateWithTimeIntervalSinceNow:0.02]];
}
static void Wait(BOOL (^done)(void)) {
    NSDate *end = [NSDate dateWithTimeIntervalSinceNow:15];
    while (!done() && end.timeIntervalSinceNow > 0)
        Pump();
    NSCAssert(done(), @"Timed out waiting for WebKit");
}
static id JS(WKWebView *web, NSString *script) {
    __block BOOL done = NO; __block id result; __block NSError *failure;
    [web evaluateJavaScript:script completionHandler:^(id value, NSError *error) { result=value;failure=error;done=YES; }];
    Wait(^BOOL { return done; });
    NSCAssert(!failure, @"JavaScript: %@",failure);
    return result;
}
struct Reply { bool done; void *json; };
static void Reply(void *context,const char *json) {
    struct Reply *reply=context; reply->json=(__bridge_retained void *)[NSString stringWithUTF8String:json];reply->done=true;
}
static NSDictionary *Command(void *view, NSDictionary *request) {
    NSData *bytes=[NSJSONSerialization dataWithJSONObject:request options:0 error:nil];
    NSString *json=[[NSString alloc] initWithData:bytes encoding:NSUTF8StringEncoding];
    __block struct Reply reply={0};
    del_browser_command(view,json.UTF8String,Reply,&reply);
    Wait(^BOOL { return reply.done; });
    NSString *text=(__bridge_transfer NSString *)reply.json;
    NSDictionary *value=[NSJSONSerialization JSONObjectWithData:[text dataUsingEncoding:NSUTF8StringEncoding] options:0 error:nil];
    NSCAssert(!value[@"error"],@"Browser command error: %@",value);
    return value[@"result"];
}
int main(int argc,char **argv) {
    @autoreleasepool {
        NSCAssert(argc==2 || argc==4,@"Pass a local fixture URL, optionally a persistence mode and profile UUID");
        [NSApplication sharedApplication];
        NSString *url=[NSString stringWithUTF8String:argv[1]];
        if (argc==4) {
            NSString *profile=[NSString stringWithUTF8String:argv[3]];
            if (strcmp(argv[2],"delete")==0) {
                // Fresh process: cleanup must work before any WebKit view/store exists.
                __block struct Reply reply={0};
                del_browser_delete_profile(profile.UTF8String,Reply,&reply);
                Wait(^BOOL { return reply.done; });
                NSString *result=(__bridge_transfer NSString *)reply.json;
                NSCAssert(![result containsString:@"error"],@"cold-start profile deletion: %@",result);
                puts("Browser profile deletion before WebKit initialization passed");
                return 0;
            }
            BOOL write=strcmp(argv[2],"write")==0;
            @autoreleasepool {
                NSWindow *window=[[NSWindow alloc] initWithContentRect:NSMakeRect(0,0,420,650) styleMask:NSWindowStyleMaskTitled backing:NSBackingStoreBuffered defer:NO];
                window.releasedWhenClosed=NO;
                void *view=del_browser_create((__bridge void *)window.contentView,url.UTF8String,profile.UTF8String);
                del_browser_frame(view,0,0,420,650,1,false,false);
                DelBrowserView *browser=(__bridge DelBrowserView *)view;
                Wait(^BOOL { return browser.web.URL && !browser.web.loading; });
                if(write) JS(browser.web,@"document.cookie='restart=retained;max-age=3600;path=/';localStorage.setItem('restart','retained')");
                else {
                    id persisted=JS(browser.web,@"({cookie:document.cookie,storage:localStorage.getItem('restart')})");
                    NSLog(@"Restart data: %@",persisted);
                    NSCAssert([persisted[@"cookie"] containsString:@"restart=retained"] && [persisted[@"storage"] isEqual:@"retained"],@"cookies and storage survive process restart");
                }
                if(write) {
                    NSDate *flush=[NSDate dateWithTimeIntervalSinceNow:8];
                    while(flush.timeIntervalSinceNow>0) [[NSRunLoop currentRunLoop] runMode:NSDefaultRunLoopMode beforeDate:[NSDate dateWithTimeIntervalSinceNow:0.02]];
                }
                del_browser_release(view);browser=nil;[window close];
            }
            NSDate *end=[NSDate dateWithTimeIntervalSinceNow:1];
            while(end.timeIntervalSinceNow>0) [[NSRunLoop currentRunLoop] runMode:NSDefaultRunLoopMode beforeDate:[NSDate dateWithTimeIntervalSinceNow:0.02]];
            puts(write ? "Browser persistence fixture saved" : "Browser cookies and storage survived process restart");
            return 0;
        }
        NSString *profileA=NSUUID.UUID.UUIDString,*profileB=NSUUID.UUID.UUIDString;
        @autoreleasepool {
        NSWindow *window=[[NSWindow alloc] initWithContentRect:NSMakeRect(0,0,900,700) styleMask:NSWindowStyleMaskTitled backing:NSBackingStoreBuffered defer:NO];
        window.releasedWhenClosed=NO;
        window.level=NSFloatingWindowLevel;
        [window orderFrontRegardless];
        NSView *parent=window.contentView;
        NSCAssert([DelBrowserURL(@"localhost:3000/test").absoluteString isEqualToString:@"http://localhost:3000/test"],@"localhost defaults to HTTP");
        NSCAssert(!DelBrowserURL(@"file:///etc/passwd"),@"no local filesystem browsing");
        NSCAssert(!DelBrowserURL(@"javascript:alert(1)"),@"no javascript URL");
        void *a=del_browser_create((__bridge void *)parent,url.UTF8String,profileA.UTF8String);
        del_browser_frame(a,0,0,420,650,1,false,false);
        DelBrowserView *browser=(__bridge DelBrowserView *)a;
        Wait(^BOOL { return browser.web.URL && !browser.web.loading && [browser.web.title isEqualToString:@"Browser fixture"]; });
        NSCAssert([JS(browser.web,@"document.querySelector('h1').textContent") isEqual:@"Browser fixture"],@"real HTTP content");
        JS(browser.web,@"document.cookie='del_test=profileA;max-age=3600;path=/';localStorage.setItem('del_test','profileA')");
        Command(a,@{@"action":@"evaluate",@"script":@"document.querySelector('input').value='hello';return {value:document.querySelector('input').value};"});
        NSDictionary *screen=Command(a,@{@"action":@"screenshot"});
        NSData *png=[[NSData alloc] initWithBase64EncodedString:screen[@"image"] options:0];
        NSCAssert(png.length>100,@"screenshot bytes");
        [png writeToFile:@"/tmp/del-browser-test.png" atomically:YES];
        Command(a,@{@"action":@"navigate",@"url":[url stringByAppendingString:@"?second"]});
        Wait(^BOOL { return !browser.web.loading && [browser.web.URL.query isEqual:@"second"]; });
        NSCAssert(browser.web.canGoBack,@"navigation history");
        Command(a,@{@"action":@"back"});
        Wait(^BOOL { return !browser.web.loading && browser.web.URL.query==nil; });
        del_browser_frame(a,6,0,414,650,1,false,false);
        NSPoint gutter=NSMakePoint(3,NSMaxY(browser.frame)-100);
        NSPoint content=NSMakePoint(80,NSMaxY(browser.frame)-100);
        NSCAssert([parent hitTest:gutter]==parent,@"sidebar resize gutter belongs to GPUI");
        NSCAssert([[parent hitTest:content] isDescendantOf:browser],@"web content receives normal clicks");
        del_browser_frame(a,6,0,414,650,1,false,true);
        NSCAssert([parent hitTest:content]==parent,@"active GPUI drags pass through WebKit");
        del_browser_frame(a,6,0,314,650,1,false,false);
        NSCAssert(NSWidth(browser.frame)==314 && [[parent hitTest:content] isDescendantOf:browser],@"resized browser restores pointer interaction");
        del_browser_hide(a); NSCAssert(browser.hidden,@"hide on tab switch");
        JS(browser.web,@"document.querySelector('input').value='preview-retains-state'");
        NSRect viewportBeforePreview = browser.web.frame;
        NSDictionary *preview=Command(a,@{@"action":@"screenshot",@"preview":@YES});
        NSData *previewPNG=[[NSData alloc] initWithBase64EncodedString:preview[@"image"] options:0];
        NSBitmapImageRep *previewBitmap=[[NSBitmapImageRep alloc] initWithData:previewPNG];
        NSCAssert(previewBitmap.pixelsWide > 0 && previewBitmap.pixelsWide <= 560 * window.backingScaleFactor,@"bounded real hidden-page thumbnail");
        NSCAssert((browser.hidden || browser.layer.opacity==0) && NSEqualRects(browser.web.frame,viewportBeforePreview),@"preview must not show or resize the actual browser");
        NSCAssert(!browser.presented && browser.accessibilityHidden && [parent hitTest:content]==parent && ![browser admitDialog],@"preview cannot expose native controls, intercept input, or open dialogs");
        NSCAssert([JS(browser.web,@"document.querySelector('input').value") isEqual:@"preview-retains-state"],@"preview preserves live page state");
        del_browser_frame(a,0,0,320,500,1.2,true,false);
        NSCAssert(!browser.hidden && browser.web.pageZoom==1.2,@"resize/reopen/zoom");
        del_browser_release(a); browser=nil;
        void *b=del_browser_create((__bridge void *)parent,url.UTF8String,profileB.UTF8String);
        del_browser_frame(b,0,0,420,650,1,false,false);
        DelBrowserView *other=(__bridge DelBrowserView *)b;
        Wait(^BOOL { return other.web.URL && !other.web.loading; });
        NSCAssert([JS(other.web,@"document.cookie.includes('del_test') || localStorage.getItem('del_test') !== null") isEqual:@NO],@"session profiles are isolated");
        del_browser_release(b);other=nil;
        a=del_browser_create((__bridge void *)parent,url.UTF8String,profileA.UTF8String);
        del_browser_frame(a,0,0,420,650,1,false,false);
        browser=(__bridge DelBrowserView *)a;
        Wait(^BOOL { return browser.web.URL && !browser.web.loading; });
        NSCAssert([JS(browser.web,@"document.cookie.includes('del_test=profileA') && localStorage.getItem('del_test')==='profileA'") isEqual:@YES],@"profile persists after closing/reopening views");
        del_browser_release(a);browser=nil;
        // A page first opened by an agent must finish frame-driven rendering
        // before the user opens its pane (video sites use this for hydration).
        a=del_browser_create((__bridge void *)parent,url.UTF8String,profileA.UTF8String);
        browser=(__bridge DelBrowserView *)a;
        Command(a,@{@"action":@"evaluate",@"script":@"return true;"});
        Wait(^BOOL { return browser.web.URL && !browser.web.loading; });
        JS(browser.web,@"window.previewReady=false;requestAnimationFrame(()=>requestAnimationFrame(()=>{document.body.style.background='rgb(0, 200, 0)';window.previewReady=true;}))");
        for (int i=0;i<3;i++) {
            Command(a,@{@"action":@"screenshot",@"preview":@YES});
            NSDate *frameDeadline=[NSDate dateWithTimeIntervalSinceNow:0.2];
            while(frameDeadline.timeIntervalSinceNow>0) Pump();
        }
        NSCAssert([JS(browser.web,@"window.previewReady") isEqual:@YES],@"hidden preview must show frame-driven page content, not a loading skeleton");
        NSDictionary *rendered=Command(a,@{@"action":@"screenshot",@"preview":@YES});
        NSBitmapImageRep *renderedBitmap=[[NSBitmapImageRep alloc] initWithData:[[NSData alloc] initWithBase64EncodedString:rendered[@"image"] options:0]];
        NSColor *renderedColor=[[renderedBitmap colorAtX:renderedBitmap.pixelsWide/2 y:renderedBitmap.pixelsHigh/2] colorUsingColorSpace:NSColorSpace.deviceRGBColorSpace];
        NSCAssert(renderedColor.greenComponent>0.6 && renderedColor.redComponent<0.1,@"preview pixels contain hydrated page content");
        del_browser_hide(a);
        NSCAssert(!browser.hidden && browser.layer.opacity==0,@"normal pane hiding must not interrupt preview rendering");
        Wait(^BOOL { return browser.hidden; });
        NSCAssert(!browser.previewRendering,@"rendering sleeps when preview sampling stops");
        Command(a,@{@"action":@"screenshot",@"preview":@YES});
        del_browser_frame(a,0,0,420,650,1,false,false);
        NSDate *leaseDeadline=[NSDate dateWithTimeIntervalSinceNow:2.2];
        while(leaseDeadline.timeIntervalSinceNow>0) Pump();
        NSCAssert(browser.presented && !browser.hidden && browser.layer.opacity==1 && !browser.accessibilityHidden,@"stale preview lease must not hide a reopened browser");
        NSCAssert([JS(browser.web,@"window.previewReady") isEqual:@YES],@"opening the browser preserves the preview's hydrated page");
        del_browser_release(a);browser=nil;
        [window close];
        }
        // Let WebKit retire closed web views before removing their data stores.
        NSDate *pause=[NSDate dateWithTimeIntervalSinceNow:0.5];
        while(pause.timeIntervalSinceNow>0) [[NSRunLoop currentRunLoop] runMode:NSDefaultRunLoopMode beforeDate:[NSDate dateWithTimeIntervalSinceNow:0.02]];
        for (NSString *profile in @[profileA,profileB]) {
            __block struct Reply reply={0};
            del_browser_delete_profile(profile.UTF8String,Reply,&reply);
            Wait(^BOOL { return reply.done; });
            NSString *result=(__bridge_transfer NSString *)reply.json;
            NSCAssert(![result containsString:@"error"],@"delete profile: %@",result);
        }
        puts("Browser navigation, JavaScript, screenshots, profile persistence/isolation and deletion passed");
    }
    return 0;
}
