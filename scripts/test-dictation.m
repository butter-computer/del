// Tests ownership/cancellation/error paths without permission prompts or recording.
#import "../crates/app/src/dictation.m"
static NSMutableArray *events;
static void Receive(void *context, const char *kind, const char *text) {
    NSCAssert(context == (__bridge void *)events,@"callback context survives until release");
    [events addObject:@{@"kind":[NSString stringWithUTF8String:kind],@"text":[NSString stringWithUTF8String:text]}];
}
static void Pump(double seconds) {
    NSDate *end=[NSDate dateWithTimeIntervalSinceNow:seconds];
    while(end.timeIntervalSinceNow>0) [[NSRunLoop currentRunLoop] runMode:NSDefaultRunLoopMode beforeDate:[NSDate dateWithTimeIntervalSinceNow:0.01]];
}
@interface TestSpeechTask : NSObject
@property(nonatomic) BOOL cancelled;
- (void)cancel;
@end
@implementation TestSpeechTask
- (void)cancel { self.cancelled = YES; }
@end
@interface TestRecognizer : NSObject
@property(nonatomic) NSUInteger starts;
@end
@implementation TestRecognizer
- (BOOL)supportsOnDeviceRecognition { return YES; }
- (SFSpeechRecognitionTask *)recognitionTaskWithRequest:(SFSpeechRecognitionRequest *)request resultHandler:(void (^)(SFSpeechRecognitionResult *, NSError *))handler {
    (void)request; (void)handler;
    self.starts++;
    return (SFSpeechRecognitionTask *)[TestSpeechTask new];
}
@end
static void TestPhrases(void) {
    DelSpeechSegment *part = [DelSpeechSegment new];
    [part updateText:@"Hello" settled:NO start:0 end:0];
    [part updateText:@"Hello there." settled:YES start:0.2 end:1.4];
    [part updateText:@"Hello there." settled:YES start:0.2 end:1.4];
    NSCAssert([part.text isEqual:@"Hello there."], @"duplicate settled callback must not append twice");
    [part updateText:@"Another" settled:NO start:0 end:0];
    [part updateText:@"Another thought." settled:YES start:2 end:3];
    NSCAssert([part.text isEqual:@"Hello there. Another thought."], @"pause must not erase preceding phrase");
    [part updateText:@"Another" settled:NO start:0 end:0];
    [part updateText:@"Another thought." settled:YES start:4 end:5];
    NSCAssert([part.text isEqual:@"Hello there. Another thought. Another thought."], @"repeated spoken phrases must be preserved");
    DelSpeechSegment *cumulative = [DelSpeechSegment new];
    [cumulative updateText:@"Hello" settled:YES start:0.2 end:1];
    [cumulative updateText:@"Hello world" settled:NO start:0 end:0];
    NSCAssert([cumulative.text isEqual:@"Hello world"], @"cumulative API responses must not duplicate the prefix");
}
static void TestContinuousRequests(void) {
    DelDictation *voice = [DelDictation new];
    TestRecognizer *recognizer = [TestRecognizer new];
    voice.recognizer = (SFSpeechRecognizer *)recognizer;
    voice.segments = [NSMutableArray new];
    voice.audioLock = [NSLock new];
    voice.engine = [AVAudioEngine new]; // never started; no microphone accessed
    AVAudioEngine *engine = voice.engine;
    [voice startSegment];
    for (NSUInteger i = 0; i < 80; i++) {
        DelSpeechSegment *old = voice.active;
        [old updateText:@"word" settled:NO start:0 end:0];
        [voice rotateSegment:old];
        NSCAssert(voice.engine == engine && !voice.ended && !voice.finishing, @"renewal must not stop capture");
        NSCAssert(voice.audioRequest == voice.active.request && voice.active != old, @"new audio must go to the renewed request");
        [old updateText:@"Word." settled:YES start:0.1 end:0.5];
        [voice retireSegment:old];
        NSCAssert(voice.segments.count == 1, @"completed requests must be released");
    }
    NSCAssert(recognizer.starts == 81 && [voice.committed componentsSeparatedByString:@" "].count == 80, @"long dictation must keep every completed request");
    DelSpeechSegment *earlier = voice.active;
    [earlier updateText:@"Earlier." settled:NO start:0 end:0];
    [voice rotateSegment:earlier];
    DelSpeechSegment *later = voice.active;
    [later updateText:@"Later." settled:NO start:0 end:0];
    voice.active = nil;
    [voice retireSegment:later];
    NSCAssert(voice.segments.count == 2, @"out-of-order final callbacks must retain speech order");
    [voice retireSegment:earlier];
    NSCAssert([voice.committed hasSuffix:@"Earlier. Later."], @"finalized text must be in spoken order");
    [voice cancel];
    [voice rotateSegment:later];
    NSCAssert(voice.engine == nil && voice.audioRequest == nil && voice.segments.count == 0 && recognizer.starts == 82, @"cancel must release audio and prevent restart");
}
int main(void) {
    @autoreleasepool {
        // A newer SDK must not compile away checks needed on the running OS.
        BOOL hasNewTap = NO;
        if (@available(macOS 27.0, *)) hasNewTap = YES;
        BOOL runningNewOS = [NSProcessInfo.processInfo isOperatingSystemAtLeastVersion:(NSOperatingSystemVersion){27, 0, 0}];
        if (hasNewTap != runningNewOS) {
            fputs("FAIL: deployment target removed the microphone API availability guard.\n", stderr);
            return 1;
        }
        [NSApplication sharedApplication];
        TestPhrases();
        TestContinuousRequests();
        events=[NSMutableArray new];
        // The raw test executable lacks usage descriptions, so it must fail safely.
        void *handle=del_dictation_start(Receive,(__bridge void *)events);
        Pump(.1);
        NSCAssert(events.count==1 && [events[0][@"kind"] isEqual:@"error"],@"missing bundle metadata is recoverable");
        del_dictation_release(handle);
        [events removeAllObjects];
        handle=del_dictation_start(Receive,(__bridge void *)events);
        del_dictation_release(handle);
        Pump(.1);
        NSCAssert(events.count==0,@"cancel before authorization cannot prompt or call freed context");
        DelDictation *dictation=[DelDictation new];
        dictation.callback=Receive;dictation.context=(__bridge void *)events;
        dictation.engine=[AVAudioEngine new];
        [dictation finish];
        NSCAssert(dictation.finishing && dictation.engine==nil,@"stop releases microphone before final recognition");
        [dictation complete];[dictation complete];
        NSCAssert(events.count==2 && [events[0][@"kind"] isEqual:@"finishing"] && [events[1][@"kind"] isEqual:@"done"],@"completion occurs once");
        puts("Dictation pauses, repeated phrases, continuous renewal, cancellation, compatibility and cleanup passed; microphone was not used.");
    }
}
