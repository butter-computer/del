#import <AppKit/AppKit.h>
#import <Carbon/Carbon.h>
#import <objc/runtime.h>

static bool (*shouldBackground)(bool) = NULL;
static void (*notifyBackground)(void) = NULL;
static IMP previousShouldTerminate = NULL;
static id backgroundActivity = nil;

void del_session_lifecycle_background(void) {
    if (!backgroundActivity) {
        // Keep the runtime responsive while hidden, without keeping the Mac awake.
        backgroundActivity = [NSProcessInfo.processInfo
            beginActivityWithOptions:NSActivityUserInitiatedAllowingIdleSystemSleep
            reason:@"Running Del chats and terminals in the background"];
    }
    if (notifyBackground) notifyBackground();
    [NSApp hide:nil];
}

void del_session_lifecycle_foreground(void) {
    if (backgroundActivity) {
        [NSProcessInfo.processInfo endActivity:backgroundActivity];
        backgroundActivity = nil;
    }
    [NSApp unhide:nil];
}

static bool delIsSystemQuit(NSAppleEventDescriptor *event) {
    OSType reason = [[event paramDescriptorForKeyword:kAEQuitReason] typeCodeValue];
    return reason == kAELogOut || reason == kAEReallyLogOut
        || reason == kAEShutDown || reason == kAERestart || reason == kAEQuitAll;
}

static NSApplicationTerminateReply delShouldTerminate(id delegate, SEL selector, NSApplication *app) {
    NSAppleEventDescriptor *event = NSAppleEventManager.sharedAppleEventManager.currentAppleEvent;
    if (shouldBackground && shouldBackground(delIsSystemQuit(event))) {
        del_session_lifecycle_background();
        return NSTerminateCancel;
    }
    if (previousShouldTerminate) {
        return ((NSApplicationTerminateReply (*)(id, SEL, NSApplication *))previousShouldTerminate)(delegate, selector, app);
    }
    return NSTerminateNow;
}

void del_show_about_panel(void) {
    [NSApp activateIgnoringOtherApps:YES];
    [NSApp orderFrontStandardAboutPanel:nil];
}

bool del_session_lifecycle_install(bool (*decide)(bool), void (*notify)(void)) {
    Class delegateClass = object_getClass(NSApp.delegate);
    if (!delegateClass) return false;
    shouldBackground = decide;
    notifyBackground = notify;
    SEL selector = @selector(applicationShouldTerminate:);
    Method existing = class_getInstanceMethod(delegateClass, selector);
    IMP implementation = existing ? method_getImplementation(existing) : NULL;
    if (implementation == (IMP)delShouldTerminate) return true;
    previousShouldTerminate = implementation;
    // Add on GPUI's delegate class; do not change AppKit's global termination
    // implementation or replace GPUI's delegate and its shutdown callbacks.
    class_replaceMethod(delegateClass, selector, (IMP)delShouldTerminate, "Q@:@");
    return true;
}
