// Run with: clang -fobjc-arc -framework AppKit -framework Carbon \
//   crates/app/tests/session_lifecycle_native.m -o /tmp/del-lifecycle-native-test \
//   && /tmp/del-lifecycle-native-test
#import "../src/session_lifecycle.m"
#include <assert.h>

int main(void) {
    @autoreleasepool {
        assert(!delIsSystemQuit(nil));
        NSAppleEventDescriptor *event = [NSAppleEventDescriptor
            appleEventWithEventClass:kCoreEventClass eventID:kAEQuitApplication
            targetDescriptor:nil returnID:kAutoGenerateReturnID transactionID:kAnyTransactionID];
        assert(!delIsSystemQuit(event));
        OSType reasons[] = {kAELogOut, kAEReallyLogOut, kAEShutDown, kAERestart, kAEQuitAll};
        for (unsigned i = 0; i < sizeof(reasons) / sizeof(reasons[0]); i++) {
            [event setParamDescriptor:[NSAppleEventDescriptor descriptorWithTypeCode:reasons[i]]
                forKeyword:kAEQuitReason];
            assert(delIsSystemQuit(event));
        }
        [event removeParamDescriptorWithKeyword:kAEQuitReason];
        assert(!delIsSystemQuit(event));
        puts("Native quit reasons: passed");
    }
}
