#import <AppKit/AppKit.h>

// AppKit routes feedback to supported hardware; unsupported devices are a no-op.
void del_selection_haptic(void) {
    NSCAssert([NSThread isMainThread], @"Haptics must run on the UI thread");
    [[NSHapticFeedbackManager defaultPerformer]
        performFeedbackPattern:NSHapticFeedbackPatternLevelChange
        performanceTime:NSHapticFeedbackPerformanceTimeNow];
}
