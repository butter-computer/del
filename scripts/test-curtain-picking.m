// Real queued GPU readbacks: deliberately let an old pick complete while its
// replacement is blocked. This must never publish the replacement as ready.
// clang -fobjc-arc -Wno-deprecated-declarations -framework Foundation -framework AppKit \
//   -framework Metal -framework CoreVideo -framework QuartzCore \
//   scripts/test-curtain-picking.m -o /tmp/del-test-curtain-picking
// /tmp/del-test-curtain-picking crates/app/src/fluid.metal
#import "../crates/app/src/fluid.m"
#include <assert.h>
#include <unistd.h>

static id<MTLCommandBuffer> frame(void *renderer,id<MTLCommandQueue> queue,id<MTLSharedEvent> gate,
    uint64_t value,unsigned style,bool pressed,bool interactive) {
    del_fluid_force_press(renderer,pressed);
    CVPixelBufferRef buffer=del_fluid_frame(renderer,384,256,1,1,12,70,100,0,0,false,style,.8,.8,.8,.14,interactive);
    assert(buffer);
    id<MTLCommandBuffer> command=[queue commandBuffer];
    if(value)[command encodeWaitForEvent:gate value:value];
    del_fluid_prepare(buffer,(__bridge void *)command);
    [command commit];CFRelease(buffer);return command;
}
static void finish(id<MTLCommandBuffer> command) {
    [command waitUntilCompleted];assert(command.status==MTLCommandBufferStatusCompleted);
}
static void completed(DelClothPick *pick) {
    for(int i=0;i<1000&&!atomic_load(&pick->ready);i++)usleep(1000);
    assert(atomic_load(&pick->ready)&&pick.succeeded);
}
int main(int argc,const char **argv) { @autoreleasepool {
    assert(argc==2);
    NSString *source=[NSString stringWithContentsOfFile:@(argv[1]) encoding:NSUTF8StringEncoding error:nil];
    assert(source);
    id<MTLDevice> device=MTLCreateSystemDefaultDevice();
    id<MTLCommandQueue> queue=[device newCommandQueue];
    id<MTLSharedEvent> gate=[device newSharedEvent];assert(gate);
    void *renderer=del_fluid_create(NULL,0,source.UTF8String,strlen(source.UTF8String));
    DelFluidRenderer *native=(__bridge DelFluidRenderer *)renderer;
    del_fluid_running(renderer,true);
    finish(frame(renderer,queue,gate,0,12,false,true));

    id<MTLCommandBuffer> first=frame(renderer,queue,gate,1,12,true,true);
    DelClothPick *old=[native valueForKey:@"pick"];assert(old&&!atomic_load(&old->ready));
    id<MTLCommandBuffer> release=frame(renderer,queue,gate,0,12,false,true);
    assert(![native valueForKey:@"pick"]);
    id<MTLCommandBuffer> second=frame(renderer,queue,gate,2,12,true,true);
    DelClothPick *current=[native valueForKey:@"pick"];
    assert(current&&current!=old&&current.buffer!=old.buffer);
    gate.signaledValue=1;finish(first);finish(release);completed(old);
    // The old GPU callback ran, but the new readback is still blocked.
    assert(!atomic_load(&current->ready));
    assert([native valueForKey:@"pick"]==current);

    // A style change cancels the current request even if its completion arrives later.
    id<MTLCommandBuffer> change=frame(renderer,queue,gate,0,2,false,true);
    assert(![native valueForKey:@"pick"]);
    gate.signaledValue=2;finish(second);finish(change);completed(current);
    assert(![native valueForKey:@"pick"]);

    // A dialog disables interaction even with a physically held mouse button.
    finish(frame(renderer,queue,gate,0,12,true,false));
    assert(![native valueForKey:@"pick"]);
    id<MTLCommandBuffer> third=frame(renderer,queue,gate,3,12,true,true);
    DelClothPick *cancelled=[native valueForKey:@"pick"];assert(cancelled);
    id<MTLCommandBuffer> modal=frame(renderer,queue,gate,0,12,true,false);
    assert(![native valueForKey:@"pick"]);
    gate.signaledValue=3;finish(third);finish(modal);completed(cancelled);
    assert(![native valueForKey:@"pick"]);
    del_fluid_release(renderer);
    puts("Overlapping picks, late completions, style changes and disabled interaction passed.");
}}
