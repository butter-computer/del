// Apple Speech + AVAudioEngine. All ownership and callbacks stay on the main thread.
#import <AppKit/AppKit.h>
#import <AVFoundation/AVFoundation.h>
#import <Speech/Speech.h>

typedef void (*DelSpeechCallback)(void *, const char *, const char *);
// Apple may reset its partial transcript after a pause without setting isFinal.
// Keep completed utterances separate from the one it is still revising.
@interface DelSpeechSegment : NSObject
@property(nonatomic, copy) NSString *prefix;
@property(nonatomic, copy) NSString *utterance;
@property(nonatomic) BOOL settled;
@property(nonatomic) BOOL finished;
@property(nonatomic) NSTimeInterval endTime;
@property(nonatomic, strong) SFSpeechAudioBufferRecognitionRequest *request;
@property(nonatomic, strong) SFSpeechRecognitionTask *task;
- (void)updateText:(NSString *)text settled:(BOOL)settled start:(NSTimeInterval)start end:(NSTimeInterval)end;
- (NSString *)text;
@end
static NSString *DelJoinSpeech(NSString *left, NSString *right) {
    if (!left.length) return right ?: @"";
    if (!right.length) return left;
    return [NSString stringWithFormat:@"%@ %@", left, right];
}
@implementation DelSpeechSegment
- (void)updateText:(NSString *)text settled:(BOOL)settled start:(NSTimeInterval)start end:(NSTimeInterval)end {
    if (!text.length) return;
    BOOL cumulative = text.length > self.utterance.length &&
        [text hasPrefix:self.utterance ?: @""];
    BOOL newUtterance = self.utterance.length && !cumulative &&
        ((self.settled && !settled) || (self.endTime > 0 && start > self.endTime + 0.1));
    if (newUtterance) self.prefix = DelJoinSpeech(self.prefix, self.utterance);
    self.utterance = text;
    self.settled = settled;
    self.endTime = end;
}
- (NSString *)text { return DelJoinSpeech(self.prefix, self.utterance); }
@end

@interface DelDictation : NSObject
@property(nonatomic) DelSpeechCallback callback;
@property(nonatomic) void *context;
@property(nonatomic, strong) AVAudioEngine *engine;
@property(nonatomic, strong) SFSpeechRecognizer *recognizer;
@property(nonatomic, strong) NSMutableArray<DelSpeechSegment *> *segments;
@property(nonatomic, strong) DelSpeechSegment *active;
@property(nonatomic, copy) NSString *committed;
@property(nonatomic, strong) NSLock *audioLock;
@property(nonatomic, strong) SFSpeechAudioBufferRecognitionRequest *audioRequest;
@property(nonatomic) BOOL tapped;
@property(nonatomic, strong) id configurationObserver;
@property(nonatomic) BOOL ended;
@property(nonatomic) BOOL finishing;
- (void)begin;
- (void)finish;
- (void)cancel;
@end

@implementation DelDictation
- (void)emit:(const char *)kind text:(NSString *)text {
    if (self.callback) self.callback(self.context, kind, (text ?: @"").UTF8String);
}
- (void)stopAudio {
    if (self.configurationObserver) {
        [NSNotificationCenter.defaultCenter removeObserver:self.configurationObserver];
        self.configurationObserver = nil;
    }
    [self.engine stop];
    if (self.tapped) { [self.engine.inputNode removeTapOnBus:0]; self.tapped = NO; }
    [self.audioLock lock];
    [self.audioRequest endAudio];
    self.audioRequest = nil;
    [self.audioLock unlock];
    self.engine = nil;
}
- (void)cancel {
    self.ended = YES;
    [self stopAudio];
    for (DelSpeechSegment *segment in self.segments) {
        [segment.request endAudio];
        [segment.task cancel];
        segment.task = nil;
    }
    [self.segments removeAllObjects];
    self.active = nil;
    self.recognizer = nil;
}
- (void)fail:(NSString *)message {
    if (self.ended) return;
    [self cancel];
    [self emit:"error" text:message];
}
- (void)complete {
    if (self.ended) return;
    [self cancel];
    [self emit:"done" text:nil];
}
- (void)begin {
    // Missing usage descriptions cause TCC to terminate the process.
    NSBundle *bundle = NSBundle.mainBundle;
    if (![bundle objectForInfoDictionaryKey:@"NSMicrophoneUsageDescription"] ||
        ![bundle objectForInfoDictionaryKey:@"NSSpeechRecognitionUsageDescription"]) {
        [self fail:@"Voice input needs the bundled Del app. Build and open del.app to enable microphone permissions."]; return;
    }
    [SFSpeechRecognizer requestAuthorization:^(SFSpeechRecognizerAuthorizationStatus status) {
        dispatch_async(dispatch_get_main_queue(), ^{
            if (self.ended) return;
            if (status != SFSpeechRecognizerAuthorizationStatusAuthorized) {
                [self fail:@"Allow Del in System Settings → Privacy & Security → Speech Recognition, then try again."]; return;
            }
            [AVCaptureDevice requestAccessForMediaType:AVMediaTypeAudio completionHandler:^(BOOL granted) {
                dispatch_async(dispatch_get_main_queue(), ^{
                    if (self.ended) return;
                    if (!granted) { [self fail:@"Allow Del in System Settings → Privacy & Security → Microphone, then try again."]; return; }
                    [self startAudio];
                });
            }];
        });
    }];
}
- (void)publishText {
    NSString *text = self.committed ?: @"";
    for (DelSpeechSegment *segment in self.segments) text = DelJoinSpeech(text, segment.text);
    [self emit:"text" text:text];
}
- (void)retireSegment:(DelSpeechSegment *)segment {
    if (self.ended || ![self.segments containsObject:segment] || segment == self.active) return;
    // At most one request drains while the next accepts microphone buffers.
    segment.finished = YES;
    [segment.task cancel];
    segment.task = nil;
    while (self.segments.firstObject.finished && self.segments.firstObject != self.active) {
        self.committed = DelJoinSpeech(self.committed, self.segments.firstObject.text);
        [self.segments removeObjectAtIndex:0];
    }
    [self publishText];
    if (self.finishing && self.segments.count == 0) [self complete];
}
- (void)rotateSegment:(DelSpeechSegment *)segment {
    if (self.ended || self.finishing || segment != self.active) return;
    [self startSegment];
    if (self.ended) return;
    [segment.request endAudio];
    __weak DelDictation *weakSelf = self;
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 3*NSEC_PER_SEC), dispatch_get_main_queue(), ^{
        [weakSelf retireSegment:segment];
    });
}
- (void)startSegment {
    if (self.ended || self.finishing) return;
    // Finish any previous drain before adding another request, bounding resources.
    for (DelSpeechSegment *old in self.segments.copy) {
        if (old != self.active) [self retireSegment:old];
    }
    DelSpeechSegment *segment = [DelSpeechSegment new];
    segment.request = [SFSpeechAudioBufferRecognitionRequest new];
    segment.request.shouldReportPartialResults = YES;
    segment.request.taskHint = SFSpeechRecognitionTaskHintDictation;
    segment.request.requiresOnDeviceRecognition = self.recognizer.supportsOnDeviceRecognition;
    if (@available(macOS 13.0, *)) segment.request.addsPunctuation = YES;
    [self.segments addObject:segment];
    self.active = segment;
    __weak DelDictation *weakSelf = self;
    __weak DelSpeechSegment *weakSegment = segment;
    segment.task = [self.recognizer recognitionTaskWithRequest:segment.request resultHandler:^(SFSpeechRecognitionResult *result, NSError *error) {
        dispatch_async(dispatch_get_main_queue(), ^{
            DelDictation *owner = weakSelf;
            DelSpeechSegment *part = weakSegment;
            if (!owner || owner.ended || !part || part.finished || ![owner.segments containsObject:part]) return;
            if (result) {
                SFTranscription *transcript = result.bestTranscription;
                SFTranscriptionSegment *first = transcript.segments.firstObject;
                SFTranscriptionSegment *last = transcript.segments.lastObject;
                [part updateText:transcript.formattedString
                         settled:result.isFinal || result.speechRecognitionMetadata != nil
                           start:first.timestamp end:last.timestamp + last.duration];
                [owner publishText];
            }
            if (result.isFinal || error) {
                if (owner.finishing) {
                    if (owner.active == part) owner.active = nil;
                    [owner retireSegment:part];
                } else if (owner.active != part) {
                    [owner retireSegment:part];
                } else if (result.isFinal || ([error.domain isEqualToString:@"kAFAssistantErrorDomain"] && error.code == 1110)) {
                    // Silence ends a recognition task, not the user's dictation.
                    [owner rotateSegment:part];
                    [owner retireSegment:part];
                } else {
                    [owner fail:[@"Dictation stopped: " stringByAppendingString:error.localizedDescription]];
                }
            }
        });
    }];
    if (!segment.task) { [self fail:@"Apple speech recognition could not start. Try again."]; return; }
    [self.audioLock lock];
    self.audioRequest = segment.request;
    [self.audioLock unlock];
    // Renew Apple's bounded requests without stopping capture or clearing text.
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 50*NSEC_PER_SEC), dispatch_get_main_queue(), ^{
        DelSpeechSegment *part = weakSegment;
        if (part) [weakSelf rotateSegment:part];
    });
}
- (void)startAudio {
    self.recognizer = [[SFSpeechRecognizer alloc] initWithLocale:NSLocale.currentLocale];
    if (!self.recognizer || !self.recognizer.available) {
        [self fail:@"Apple speech recognition is unavailable for your current language. Check your connection and macOS language settings."]; return;
    }
    self.recognizer.queue = NSOperationQueue.mainQueue;
    self.segments = [NSMutableArray new];
    self.audioLock = [NSLock new];
    __weak DelDictation *weakSelf = self;
    @try {
        self.engine = [AVAudioEngine new];
        AVAudioFormat *format = [self.engine.inputNode outputFormatForBus:0];
        if (!format.channelCount || !format.sampleRate) { [self fail:@"No microphone is available. Connect or select an input device in macOS Sound settings."]; return; }
        [self startSegment];
        if (self.ended) return;
        NSString *failure = [self connectInput];
        if (failure) { [self fail:failure]; return; }
    } @catch (NSException *exception) {
        NSLog(@"Del microphone setup failed (%@): %@", exception.name, exception.reason);
        [self fail:@"Could not start microphone. Check your macOS audio input and try again."]; return;
    }
    // Connecting or removing a device (such as AirPods) stops the engine and
    // may change the input format. Without this, the UI would keep listening.
    self.configurationObserver = [NSNotificationCenter.defaultCenter
        addObserverForName:AVAudioEngineConfigurationChangeNotification object:self.engine
        queue:NSOperationQueue.mainQueue usingBlock:^(NSNotification *note) {
            (void)note;
            [weakSelf inputChanged];
        }];
    [self emit:"listening" text:nil];
}
// Install the microphone tap in the input's current format and start the
// engine. Returns a user-facing failure, or nil.
- (NSString *)connectInput {
    AVAudioInputNode *input = self.engine.inputNode;
    if (self.tapped) { [input removeTapOnBus:0]; self.tapped = NO; }
    AVAudioFormat *format = [input outputFormatForBus:0];
    if (!format.channelCount || !format.sampleRate) return @"No microphone is available. Connect or select an input device in macOS Sound settings.";
    __weak DelDictation *weakSelf = self;
    AVAudioNodeTapBlock tap = ^(AVAudioPCMBuffer *buffer, AVAudioTime *time) {
        (void)time;
        DelDictation *owner = weakSelf;
        [owner.audioLock lock];
        [owner.audioRequest appendAudioPCMBuffer:buffer];
        [owner.audioLock unlock];
    };
    // Use the long-supported API on every supported OS. New SDKs deprecate it
    // but it remains available; no macOS 27-only selector is needed here.
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
    [input installTapOnBus:0 bufferSize:1024 format:format block:tap];
#pragma clang diagnostic pop
    self.tapped = YES;
    [self.engine prepare];
    NSError *error = nil;
    if (![self.engine startAndReturnError:&error]) return error.localizedDescription ?: @"Could not start microphone.";
    return nil;
}
- (void)inputChanged {
    if (self.ended || self.finishing || !self.engine) return;
    NSString *failure = nil;
    @try {
        [self.engine stop];
        failure = [self connectInput];
    } @catch (NSException *exception) {
        NSLog(@"Del microphone reconfiguration failed (%@): %@", exception.name, exception.reason);
        failure = @"Could not restart the microphone after the input device changed.";
    }
    if (failure) { [self fail:[@"Dictation stopped: " stringByAppendingString:failure]]; return; }
    // The new device can use another format; give it a fresh request while
    // the previous one finishes what it already heard.
    if (self.active) [self rotateSegment:self.active];
}
- (void)finish {
    if (self.ended || self.finishing) return;
    if (!self.engine) { [self complete]; return; }
    self.finishing = YES;
    [self stopAudio];
    [self emit:"finishing" text:nil];
    __weak DelDictation *weakSelf = self;
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 3*NSEC_PER_SEC), dispatch_get_main_queue(), ^{
        [weakSelf complete];
    });
}
@end

void *del_dictation_start(DelSpeechCallback callback, void *context) {
    if (!NSThread.isMainThread) return NULL;
    DelDictation *dictation = [DelDictation new];
    dictation.callback = callback; dictation.context = context;
    // Return the handle before any event can reach Rust.
    dispatch_async(dispatch_get_main_queue(), ^{ if (!dictation.ended) [dictation begin]; });
    return (__bridge_retained void *)dictation;
}
void del_dictation_finish(void *pointer) {
    if (NSThread.isMainThread && pointer) [(__bridge DelDictation *)pointer finish];
}
void del_dictation_release(void *pointer) {
    if (!NSThread.isMainThread || !pointer) return;
    DelDictation *dictation = (__bridge_transfer DelDictation *)pointer;
    dictation.callback = NULL; dictation.context = NULL;
    [dictation cancel];
}
